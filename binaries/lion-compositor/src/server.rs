//! The compositor assembly: bring-up, the LDP server, the accept loop.
//!
//! [`Compositor::headless`] performs the whole pipeline bring-up on
//! the mock device — output selection, primary-plane match, dual
//! framebuffer registration, the applied `ALLOW_MODESET` enable
//! commit (the Phase 25 serve choreography — the same request
//! sequence the DRM path submits for real), and the advance to the
//! first page flip that latches the timeline (the scheduler's PLL
//! anchor) — then stands up an [`ldp_server::Server`] advertising
//! `ldp.core.compositor`, `ldp.core.shm`, and `ldp.core.output`, and
//! listens on an abstract-namespace Unix socket. Every accepted
//! session gets a [`CompositorDispatcher`] over the shared world.
//!
//! [`Compositor::drm`] is the real thing: DRM-Master takeover, kernel
//! dumb buffers mapped for CPU composition, the applied enable commit
//! on real hardware, and [`Compositor::serve_kms`] — the poll loop
//! that watches the listening socket *and* the DRM fd (flip landings,
//! topology events) until shutdown, then tears the pipeline down
//! honestly (plane off, CRTC off, unbind, objects released, master
//! dropped). On machines without DRM nodes the bring-up fails typed —
//! the headless path is the explicit fallback, never a silent one.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use ldp_compositor::scheduler::SchedulerConfig;
use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::error::{LdpError, Result};
use ldp_core::limits::Limits;
use ldp_display::backend::KmsBackend;
use ldp_display::driver::{DisplayDriver, DrmDriver};
use ldp_display::drm::DrmBackend;
use ldp_display::fb::FbSpec;
use ldp_display::hotplug::topology_statuses;
use ldp_display::ids::PlaneId;
use ldp_display::mode::ModeType;
use ldp_display::plane::PlaneType;
use ldp_display::serve;
use ldp_display::serve::ModeSize;
use ldp_display::MockDevice;
use ldp_server::{GlobalAdvert, Server, ServerConfig};
use ldp_transport::{TransportListener, UnixAddr};

use crate::dispatch::CompositorDispatcher;
use crate::output::OutputGlobal;
use crate::rearrange::allocate_drm_scanout;
use crate::scene::{DrmLease, ScanoutChain, Scene, Shared, World};
use crate::sys;

/// How the binary drives the compositor.
// The bools are operator switches — each names exactly one CLI flag
// (a collapsed enum would invent combinations no operator can reach).
#[allow(clippy::struct_excessive_bools)]
pub struct CompositorConfig {
    /// Abstract-namespace socket name (without the leading NUL).
    pub socket: String,
    /// Optional raw-frame dump directory (`--dump`).
    pub dump: Option<std::path::PathBuf>,
    /// The frame scheduler's policy knobs.
    pub scheduler: SchedulerConfig,
    /// The renderer selection: Auto (hardware first, the default),
    /// forced GL, or forced software — `--renderer`.
    pub renderer: ldp_renderer::gles::RendererChoice,
    /// The Liquid effects selection (Phase 27, `--effects`): Auto
    /// resolves the quality tier from the machine (GL serves High,
    /// software on a phone-sized output serves Medium, software on a
    /// desktop-sized output serves Low, the headless mock stays
    /// Minimal), or a fixed tier. The **library default is Minimal**
    /// — plain Phase 26 pixels, byte-identical — so the equivalence
    /// corpora keep their oracles; the CLI defaults to `auto`.
    pub effects: ldp_renderer::EffectChoice,
    /// An injected GL backend for the equivalence suites (the whole
    /// compositor driven through the reference evaluator); `None` in
    /// every production path.
    pub renderer_api: Option<Box<dyn ldp_renderer::gles::api::GlesApi>>,
    /// The bring-up selection report ("renderer: …"), filled at
    /// bring-up and printed unless quiet — the honest line.
    pub renderer_report: String,
    /// The scanout store: `true` builds the *mapped* store over two
    /// anonymous mappings (the Phase 25 equivalence suites — the real
    /// delivery path, pitch-honoring writes, on CI machines); `false`
    /// (the default, every production headless path) keeps the shadow
    /// word buffers.
    pub scanout_mapped: bool,
    /// The positioning shell (Phase 28): toplevel placement and the
    /// system dock. The **library default keeps the dock off** (plain
    /// pixels, byte-identical) so the equivalence corpora keep their
    /// oracles; the CLI's `--dock auto` (its default) serves the home
    /// dock.
    pub shell: crate::shell::ShellConfig,
    /// The forced output size (Phase 30, `--resolution WxH`): the
    /// bring-up and every hotplug migration prefer a mode of exactly
    /// this size; a display that does not offer it fails bring-up
    /// honestly (the offered menu in the error) and a migration falls
    /// back to the unsized doctrine. `None` (the default) is the
    /// plain preferred-mode doctrine.
    pub resolution: Option<ModeSize>,
    /// The mode foundry's pour (Phase 42, `--synth WxH[@HZ]`; Phase
    /// 46 — the family grammar `--synth WxH[@HZ][:family]`): a VESA
    /// timing synthesized for the asked size and refresh — CVT-RB
    /// (the default), CVT-RB2, RB2-video, CVT standard CRT blanking,
    /// or GTF — the custom-resolution machine the giants' decades
    /// built (`xrandr --newmode`'s arithmetic), poured as a
    /// **user-defined mode** (`USERDEF` type bit, the kernel's own
    /// vocabulary). The connector's EDID range limits gate the pour
    /// (a clock above the declared ceiling refuses bring-up, naming
    /// it — never a silent clamp); the engine validates the pour
    /// again at commit time; the poured mode joins the protocol
    /// face's advertised list. `None` (the default) keeps the
    /// firmware's list as the truth (the `--resolution` doctrine).
    /// Mutually exclusive with `resolution` (the CLI refuses both).
    pub synth: Option<ldp_display::timing::SynthRequest>,
    /// The multi-output doctrine (Phase 31, `--outputs`): `false` (the
    /// **library default**) serves exactly one pipeline — the Phase
    /// 25/26 single-output behavior, byte-identical, so every
    /// equivalence oracle keeps its meaning; `true` serves every
    /// pipeline the allocator finds (several CRTCs, several scanout
    /// chains, one logical desktop that extends and re-flows with the
    /// topology).
    pub multi_output: bool,
    /// The arrangement of the served outputs (Phase 37,
    /// `--outputs mirror`): `Extended` (the default) lays the outputs
    /// left-to-right into one logical desktop — the Phase 31 bytes;
    /// `Mirrored` places them all at the layout origin — every
    /// display shows the same desktop (the primary's bounds), each
    /// cropped to its own mode. The overlap is itself the protocol
    /// signal: a client that binds two outputs at the same position
    /// knows they are clones.
    pub arrangement: crate::scene::OutputArrangement,
    /// The per-output scale factors (Phase 37, `--scale F1,F2,…`):
    /// output *i* advertises factor *i*, extras reusing the last
    /// entry (the stretch rule — a hotplug newcomer never lacks a
    /// factor). Empty (the default) applies `scale` to every output
    /// — the Phase 31 doctrine, byte-identical.
    pub output_scales: Vec<ldp_core::scale::ScaleFactor>,
    /// The output scale factor (Phase 31, `--scale F`): every served
    /// output advertises it (`output.scale`, Q8.8 — the fractional
    /// HiDPI truth, no longer pinned to 1.0) and the positioning
    /// shell resolves its layout doctrine on the *logical* canvas —
    /// a 1920x1080 panel at 2x serves a 960x540 phone-density
    /// desktop. The renderer stays native (the framebuffer is the
    /// mode's pixels); identity (the default) keeps every Phase 28
    /// byte.
    pub scale: ldp_core::scale::ScaleFactor,
    /// The forced HDR pipeline (Phase 31, `--hdr`): every output
    /// advertises PQ with a 600-nit peak over `output.hdr_caps`, and
    /// the output-mode controller takes the composite onto the PQ
    /// canvas when the stack carries HDR content (per-surface
    /// `set_color`/`set_hdr_metadata`), with the dwell hysteresis.
    /// `false` (the default) is the honest SDR panel.
    pub hdr: bool,
    /// The operator's panel-peak truth (Phase 38, `--hdr-peak NITS`):
    /// the peak the panel actually *sustains* — the honesty knob for
    /// the bloated-peak quirk class (panels that advertise more than
    /// they deliver). The advertisement (`hdr_caps`), the canvas's
    /// negotiated ceiling, and every HDR layer's tone policy all
    /// carry the *effective* peak this concludes. `None` (the
    /// default) keeps the `--hdr` pipeline's own 600-nit advertisement.
    /// Only meaningful with `hdr`.
    pub hdr_peak: Option<u32>,
    /// The idle ladder's inactivity timeout (Phase 31, `--idle MS`):
    /// Dimmed at the timeout (the backlight ramp), Off at twice it
    /// (the DPMS blank + the scheduler park), Suspend at four times
    /// it (the session layer's vocabulary). `None` (the default)
    /// never sleeps.
    pub idle_ms: Option<u32>,
    /// Whether panel self-refresh serves (Phase 35): `true` by
    /// default (the sleeping-panel doctrine — the panel holds its
    /// own GRAM while the content is static, the display engine
    /// goes quiet); `--no-psr` is the operator's escape for panels
    /// whose self-refresh flickers (the quirk table's first row).
    pub psr: bool,
    /// The evdev input path (Phase 33, `--input auto|on|off`): `Auto`
    /// (the default) serves real devices when the DRM mode serves and
    /// stays off headless (the CI byte-exactness doctrine — the mock
    /// path's pixels never change because input never arrives); `On`
    /// opens the machine's `/dev/input` devices unconditionally; `Off`
    /// never routes input.
    pub input: crate::input::InputMode,
    /// Adaptive sync (Phase 31, `--vrr`): every VRR-capable output's
    /// flips carry the CRTC's VRR enablement (the panel stretches its
    /// scanout clock inside the advertised window — the mock's
    /// timeline clamps exactly like the kernel), and the deadline
    /// scheduler's window widens by the ldp-vrr policy's decision
    /// (the deadline doctrine: `max - nominal`, the stretch a late
    /// commit may consume). `false` (the default) keeps the fixed
    /// nominal grid — the Phase 25 bytes.
    pub vrr: bool,
    /// The mixed-desktop escape (Phase 41, `--vrr-uniform`): when the
    /// desktop spans a VRR panel and a fixed panel and the operator
    /// has watched the fixed sibling flicker (the cross-CRTC clock
    /// coupling the quirk table's `vrr-sibling-flicker` row names),
    /// one uniform fixed desktop serves across the seam — the
    /// pre-2020 driver doctrine, the honest escape. `false` (the
    /// default) is the modern doctrine: per-output VRR, the
    /// per-display behavior Windows serves. Only meaningful with
    /// `vrr`.
    pub vrr_uniform: bool,
    /// The operator's honest floor (Phase 41, `--vrr-floor N`): the
    /// effective minimum refresh *rate* the panel sustains — the
    /// honesty knob for the `vrr-floor-flicker` quirk class (panels
    /// whose advertised range flickers at the bottom; the same
    /// doctrine as `--hdr-peak`: the advertisement is the bloated
    /// side). The advertised window clamps to it at bring-up — every
    /// consumer (the scheduler's widening, the `output.vrr` event,
    /// the LFC cadence) sees the effective truth. `None` (the
    /// default) keeps the panel's own advertisement.
    pub vrr_floor: Option<u32>,
    /// The per-output floors (Phase 41, `--vrr-floor F1,F2,…`):
    /// output *i* takes floor *i*, extras reusing the last entry
    /// (the stretch rule — the `--scale` grammar's mirror); `0` is
    /// that output's passthrough. Empty (the default) applies
    /// `vrr_floor` to every output.
    pub vrr_floors: Vec<u32>,
    /// The compositor-owned transitions (Phase 47, `--transitions`):
    /// the system-level motion vocabulary — the window-open fade over
    /// the spring engine, advanced at the pump's cadence (the
    /// frame-callback economy drives it; the serve loop's poll tick
    /// wakes a desktop whose clients all sleep). The **library
    /// default is off** — plain pixels, byte-identical, so every
    /// equivalence oracle keeps its meaning; the CLI's `--transitions`
    /// opts the choreography in for the served desktop.
    pub transitions: bool,
    /// The spaces count (Phase 49, `--workspaces N`): how many
    /// desktops the one seat carries. The **library default is four**
    /// — the desktop doctrine (a window can move to spaces 0..3 and
    /// the desktop answers honestly); `0` promotes to one (the
    /// single-space world, the machine's own doctrine).
    pub workspaces: u32,
}

impl Clone for CompositorConfig {
    fn clone(&self) -> Self {
        Self {
            socket: self.socket.clone(),
            dump: self.dump.clone(),
            scheduler: self.scheduler,
            renderer: self.renderer,
            effects: self.effects,
            input: self.input,
            // The injected backend is a move-once resource: it is
            // consumed at bring-up; clones carry none.
            renderer_api: None,
            renderer_report: self.renderer_report.clone(),
            scanout_mapped: self.scanout_mapped,
            shell: self.shell,
            resolution: self.resolution,
            synth: self.synth,
            multi_output: self.multi_output,
            arrangement: self.arrangement,
            output_scales: self.output_scales.clone(),
            scale: self.scale,
            hdr: self.hdr,
            hdr_peak: self.hdr_peak,
            vrr: self.vrr,
            vrr_uniform: self.vrr_uniform,
            vrr_floor: self.vrr_floor,
            vrr_floors: self.vrr_floors.clone(),
            idle_ms: self.idle_ms,
            psr: self.psr,
            transitions: self.transitions,
            workspaces: self.workspaces,
        }
    }
}

impl core::fmt::Debug for CompositorConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CompositorConfig")
            .field("socket", &self.socket)
            .field("dump", &self.dump)
            .field("renderer", &self.renderer)
            .field("effects", &self.effects)
            .field("renderer_report", &self.renderer_report)
            .field("scanout_mapped", &self.scanout_mapped)
            .field("shell", &self.shell)
            .field("resolution", &self.resolution)
            .field("synth", &self.synth)
            .field("multi_output", &self.multi_output)
            .field("arrangement", &self.arrangement)
            .field("output_scales", &self.output_scales)
            .field("scale", &self.scale)
            .field("hdr", &self.hdr)
            .field("hdr_peak", &self.hdr_peak)
            .field("vrr", &self.vrr)
            .field("vrr_uniform", &self.vrr_uniform)
            .field("vrr_floor", &self.vrr_floor)
            .field("vrr_floors", &self.vrr_floors)
            .field("idle_ms", &self.idle_ms)
            .field("transitions", &self.transitions)
            .field("workspaces", &self.workspaces)
            .finish_non_exhaustive()
    }
}

impl Default for CompositorConfig {
    fn default() -> Self {
        CompositorConfig {
            socket: format!("lion-compositor-{}", std::process::id()),
            dump: None,
            scheduler: SchedulerConfig::default(),
            renderer: ldp_renderer::gles::RendererChoice::Auto,
            // The library default stays Minimal (plain Phase 26
            // pixels): the equivalence corpora and every programmatic
            // construction keep their byte-exact oracles. The CLI's
            // `--effects auto` (its default) opts real serves in.
            effects: ldp_renderer::EffectChoice::Tier(ldp_renderer::EffectTier::Minimal),
            renderer_api: None,
            renderer_report: String::new(),
            scanout_mapped: false,
            // The library default keeps the dock off (plain pixels);
            // the CLI's `--dock auto` opts real serves in.
            shell: crate::shell::ShellConfig::default(),
            // The library default is the plain preferred-mode doctrine;
            // the CLI's `--resolution` opts the sized one in.
            resolution: None,
            // The library default keeps the firmware's list as the
            // truth; the CLI's `--synth` opts the foundry in.
            synth: None,
            // The library default is the single-output doctrine (the
            // Phase 25/26 behavior, byte-identical); the CLI's
            // `--outputs auto` (its default) opts the multi doctrine
            // in for real serves.
            multi_output: false,
            // The library default extends (the Phase 31 bytes); the
            // CLI's `--outputs mirror` opts the clone doctrine in.
            arrangement: crate::scene::OutputArrangement::default(),
            // The library default is the single-factor doctrine (the
            // Phase 31 bytes); the CLI's `--scale F1,F2,…` opts the
            // per-output factors in.
            output_scales: Vec::new(),
            // The library default is identity scale (the Phase 28
            // geometry byte-exact); the CLI's `--scale` opts a
            // fractional one in.
            scale: ldp_core::scale::ScaleFactor::IDENTITY,
            // The library default is the honest SDR panel; the CLI's
            // `--hdr` opts the forced PQ pipeline in.
            hdr: false,
            // The library default keeps the pipeline's own peak
            // advertisement; the CLI's `--hdr-peak N` caps it.
            hdr_peak: None,
            // The library default is the fixed nominal grid; the
            // CLI's `--vrr` opts adaptive sync in.
            vrr: false,
            // The library default is the modern per-output doctrine;
            // the CLI's `--vrr-uniform` opts the uniform escape in.
            vrr_uniform: false,
            // The library default keeps the panel's own advertised
            // window; the CLI's `--vrr-floor` clamps it honestly.
            vrr_floor: None,
            // The library default is the blanket doctrine; the CLI's
            // `--vrr-floor F1,F2,…` opts the per-output floors in.
            vrr_floors: Vec::new(),
            // The library default never sleeps; the CLI's `--idle`
            // opts the ladder in.
            idle_ms: None,
            // The library default sleeps the panel when the content
            // is static (the Phase 35 doctrine); the CLI's `--no-psr`
            // is the operator's escape.
            psr: true,
            // The library default is Auto (real devices with DRM,
            // off headless — the byte-exactness doctrine); the CLI's
            // `--input on` opts the devices in unconditionally.
            input: crate::input::InputMode::Auto,
            // The library default keeps the desktop's pixels plain
            // (every equivalence oracle byte-identical); the CLI's
            // `--transitions` opts the compositor-owned choreography
            // in.
            transitions: false,
            // The library default is four spaces — the desktop
            // doctrine; the CLI's `--workspaces N` opts another count
            // in.
            workspaces: 4,
        }
    }
}

/// The adaptive-sync bring-up (Phase 41's quirk ledger): the
/// arming, the uniform doctrine, and the floor overrides — one
/// bundle so the world's bring-up reads the operator's VRR truth
/// as a unit.
#[derive(Clone, Debug, Default)]
pub struct VrrSetup {
    /// `--vrr`: adaptive sync armed on every VRR-capable output.
    pub enabled: bool,
    /// `--vrr-uniform`: the mixed-desktop escape — one fixed desktop
    /// across the seam when any output lacks the window.
    pub uniform: bool,
    /// `--vrr-floor F`: the blanket floor (every output).
    pub floor: Option<u32>,
    /// `--vrr-floor F1,F2,…`: the per-output floors (the stretch
    /// rule — entry *i*, the last entry for extras).
    pub floors: Vec<u32>,
}

impl VrrSetup {
    /// From the config's VRR fields (the bring-up paths' shared view).
    #[must_use]
    pub fn from_config(config: &CompositorConfig) -> VrrSetup {
        VrrSetup {
            enabled: config.vrr,
            uniform: config.vrr_uniform,
            floor: config.vrr_floor,
            floors: config.vrr_floors.clone(),
        }
    }

    /// The floor for output `index` (the stretch rule: entry *i*,
    /// the last entry for extras, the blanket value when the list is
    /// empty, `0` — passthrough — when nothing was asked).
    #[must_use]
    pub fn floor_for(&self, index: usize) -> u32 {
        self.floors
            .get(index)
            .or(self.floors.last())
            .copied()
            .unwrap_or(self.floor.unwrap_or(0))
    }
}

/// A running compositor.
pub struct Compositor {
    /// The shared world (render + dispatch state).
    pub shared: Arc<Shared>,
    /// The protocol server.
    pub server: Server,
    /// The listening socket.
    pub listener: TransportListener,
    /// The bound abstract address.
    pub addr: UnixAddr,
}

/// The headless bring-up's slot selection (Phase 31's split of
/// `headless`): the multi-output doctrine asks the allocator for every
/// pipeline the mock device can serve at once (a forced size riding
/// per-connector, the total miss the honest typed failure with the
/// union menu); the single-output doctrine stays Phase 25 verbatim
/// (the sized doctrine's miss is the operator's offered menu).
///
/// # Errors
///
/// Bring-up failures of the mock pipeline — the typed offered-menu
/// miss, plane mismatch, the enable commit rejected.
// The bring-up narrative reads best unsplit: pipeline selection,
// the scanout store, the enable choreography, and the capability
// walks (planes, PSR) in one place.
#[allow(clippy::too_many_lines)]
fn headless_slots(
    device: &mut MockDevice,
    config: &CompositorConfig,
) -> Result<Vec<crate::scene::OutputSlot>> {
    let slots: Vec<crate::scene::OutputSlot> = if config.multi_output {
        // The multi-output doctrine (Phase 31): the allocator's
        // answer — every pipeline the device can serve at once. A
        // forced size rides per-connector; a total miss is the
        // honest typed failure with the union menu. Phase 42: the
        // foundry replaces the sized doctrine when armed — every
        // connected connector gets its own pour, each gated by its
        // own EDID range limits (the ceiling-refusing connector is
        // skipped honestly; nothing serving surfaces the refusal).
        let selection = match (config.resolution, config.synth) {
            (_, Some(synth)) => serve::select_pipelines_synth(device, synth)
                .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(format!("synth {e}")))))?,
            (Some(size), None) => serve::select_pipelines(device, Some(size)).map_err(|e| {
                LdpError::Io(Arc::new(std::io::Error::other(format!("resolution {e}"))))
            })?,
            (None, None) => serve::select_pipelines(device, None)
                .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e.to_string()))))?,
        };
        let mut slots = Vec::with_capacity(selection.len());
        for pipeline in &selection {
            slots.push(light_mock_slot(
                device,
                pipeline,
                config.resolution,
                config.scanout_mapped,
            )?);
        }
        slots
    } else {
        // The single-output doctrine — Phase 25 verbatim.
        let mut output = OutputGlobal::from_mock(device, (0, 0)).ok_or_else(|| {
            LdpError::Io(Arc::new(std::io::Error::other(
                "no connected output with modes on the mock device",
            )))
        })?;
        // Phase 42 — the mode foundry's pour: `--synth` replaces the
        // sized doctrine with a pour onto *this* connector (the one
        // the single doctrine froze — its own EDID range limits gate
        // the pour; the typed refusal names the ceiling, never a
        // silent clamp), and the protocol face *adopts* the poured
        // mode — it joins the advertised list, `xrandr --addmode`'s
        // story, protocol-side.
        if let Some(synth) = config.synth {
            let family = synth.family;
            let mode = ldp_display::pour(synth).ok_or_else(|| {
                LdpError::Io(Arc::new(std::io::Error::other(format!(
                    "synth: degenerate {}x{} ask for {}",
                    synth.width, synth.height, family
                ))))
            })?;
            if let Some(declared) = output
                .connector
                .identity
                .as_ref()
                .and_then(|id| id.ranges.map(|r| r.max_clock_khz))
                .filter(|cap| mode.clock_khz > *cap)
            {
                return Err(LdpError::Io(Arc::new(std::io::Error::other(format!(
                    "synth: the {}x{} pour clocks {} kHz over the panel's declared {} kHz ceiling",
                    synth.width, synth.height, mode.clock_khz, declared
                )))));
            }
            output.adopt_mode(&mode);
        } else if let Some(size) = config.resolution {
            // The sized doctrine (Phase 30): a forced size re-points the
            // active mode — the mock's menu is small and the miss is the
            // honest typed failure (the operator asked for something the
            // deterministic panel does not carry).
            if !output.select_size(size) {
                let mut offered: Vec<(u32, u32)> = output
                    .connector
                    .modes
                    .iter()
                    .map(|m| (u32::from(m.hdisplay), u32::from(m.vdisplay)))
                    .collect();
                offered.sort_unstable();
                offered.dedup();
                let menu = offered
                    .iter()
                    .map(|(w, h)| format!("{w}x{h}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(LdpError::Io(Arc::new(std::io::Error::other(format!(
                    "no {} mode on the mock panel (offered: {menu})",
                    size.as_str()
                )))));
            }
        }
        let plane = primary_plane_for(device, output.crtc).ok_or_else(|| {
            LdpError::Io(Arc::new(std::io::Error::other(
                "no primary plane feeds the chosen CRTC",
            )))
        })?;
        let mode = output.mode();
        let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
        let pitch = w * 4;
        // Dual scanout chain; mock GEM handles are minted by the FB
        // allocator (nonzero, unique per buffer).
        let fb0 = device
            .add_fb(
                &FbSpec::single(w, h, FourCC::XRGB8888, 1, pitch, 0),
                Some(Modifier::LINEAR),
            )
            .map_err(|e| display_bringup(&e))?;
        let fb1 = device
            .add_fb(
                &FbSpec::single(w, h, FourCC::XRGB8888, 2, pitch, 0),
                Some(Modifier::LINEAR),
            )
            .map_err(|e| display_bringup(&e))?;
        // The applied enable commit — the serve choreography over the
        // mock (the identical request the DRM path submits for real).
        let pipeline = serve::Pipeline {
            connector: output.connector.id,
            crtc: output.crtc,
            plane,
            mode: mode.clone(),
        };
        serve::enable(device, &pipeline, fb0).map_err(|e| display_bringup(&e))?;

        // The scanout store: the shadow pair by default; the mapped
        // pair (anonymous mappings at the mode's pitch) for the
        // equivalence suites — the real delivery path on CI.
        let pixels = usize::try_from(w * h).unwrap_or(0);
        let scanout = if config.scanout_mapped {
            let byte_len = usize::try_from(u64::from(pitch) * u64::from(h)).unwrap_or(0);
            let maps = [
                ldp_display::drm::sys::anon_mapping(byte_len).map_err(|e| display_bringup(&e))?,
                ldp_display::drm::sys::anon_mapping(byte_len).map_err(|e| display_bringup(&e))?,
            ];
            ScanoutChain::mapped(maps, pitch as usize, w, h, [fb0, fb1])
        } else {
            ScanoutChain::shadow(pixels, [fb0, fb1])
        };
        let planes = plane_inventory_for(&*device, pipeline.crtc);
        let psr_capable = psr_capable_for(&*device, pipeline.connector);
        vec![crate::scene::OutputSlot {
            output,
            crtc: pipeline.crtc,
            plane,
            scanout,
            lease: None,
            flips: 0,
            pending: ldp_core::geometry::Region::new(),
            owes: false,
            planes,
            active_planes: Vec::new(),
            all_planes: false,
            display: Vec::new(),
            psr: ldp_power::psr::PsrMachine::default_machine(),
            psr_capable,
            psr_live: false,
        }]
    };
    Ok(slots)
}

/// Whether a connector carries the `panel self refresh` property
/// (Phase 35's bring-up walk): the name-resolved probe through the
/// backend's own property tables — the mock's connectors all do; a
/// real eDP panel reports it, a DP/HDMI sink usually does not. An
/// incapable connector simply never engages (the machine is still
/// born, but `psr_tick` skips it — the honest no-op, never a retry
/// loop into a device that said no once).
fn psr_capable_for(device: &dyn KmsBackend, connector: ldp_display::ids::ConnectorId) -> bool {
    let Ok(props) = device.object_properties(ldp_display::ids::AnyId::Connector(connector)) else {
        return false;
    };
    props.entries.iter().any(|(id, _)| {
        device
            .property(*id)
            .is_ok_and(|def| def.name.as_str() == ldp_display::props::prop::PANEL_SELF_REFRESH)
    })
}

/// Collect one pipeline's plane inventory (Phase 34): the per-CRTC
/// capability walk — primary, overlays above it, the reserved cursor —
/// with the honest degrade: a device whose walk fails serves the
/// pre-Phase-34 composite-only doctrine (an empty inventory), the
/// refusal on stderr where the operator reads it.
fn plane_inventory_for(
    device: &dyn KmsBackend,
    crtc: ldp_display::ids::CrtcId,
) -> ldp_planes::PlaneInventory {
    let index = device.topology().ok().and_then(|t| t.crtc_index(crtc));
    if let Some(i) = index {
        match ldp_planes::PlaneInventory::collect(device, i) {
            Ok(inv) => return inv,
            Err(e) => eprintln!(
                "planes: inventory walk failed on CRTC {} — serving composite-only ({e})",
                crtc.raw()
            ),
        }
    } else {
        eprintln!(
            "planes: CRTC {} not in the topology — serving composite-only",
            crtc.raw()
        );
    }
    ldp_planes::PlaneInventory::default()
}

impl Compositor {
    /// Bring up the whole pipeline and listen (the mock device).
    ///
    /// The config is consumed: the injected renderer backend (when
    /// present) is a move-once resource.
    ///
    /// # Errors
    ///
    /// Bring-up failures (no connected connector, plane mismatch, the
    /// enable commit rejected, a forced-GL renderer without hardware)
    /// and bind failures — all fatal at startup, reported as an
    /// I/O-class [`LdpError`] with context.
    pub fn headless(config: CompositorConfig) -> Result<Compositor> {
        let mut config = config;
        // ---- the display pipeline ----------------------------------
        let mut device = MockDevice::laptop_dual();
        let slots = headless_slots(&mut device, &config)?;

        // ---- the world ---------------------------------------------
        let mut world = build_world(
            Box::new(device),
            slots,
            config.scheduler,
            config.dump.clone(),
            config.renderer,
            config.renderer_api.take(),
            config.effects,
            config.shell,
            config.resolution,
            config.synth,
            config.scale,
            config.output_scales.clone(),
            config.arrangement,
            config.hdr,
            config.hdr_peak,
            VrrSetup::from_config(&config),
            config.idle_ms,
            config.psr,
            config.multi_output,
            config.input,
            true,
        )?;
        // Phase 47: the compositor-owned choreography switch — the
        // library default is off (byte-exact pixels); the operator's
        // `--transitions` opts the window-open fades in.
        world.scene.transitions.enabled = config.transitions;
        // Phase 49: the states arm's space count — the operator's
        // `--workspaces N` (the library default is four). Nothing
        // reflows at bring-up (no windows exist yet).
        world.spaces.set_count(config.workspaces);
        // Latch the timeline: advance to the bring-up flip's landing
        // and feed the scheduler its first anchor (the PLL's t=0).
        world.land_bringup_flip();
        finish_assembly(world, &config.socket)
    }

    /// Bring up the whole pipeline and listen — the real DRM device.
    ///
    /// The Phase 25 serve path: master rights, kernel dumb buffers
    /// mapped for CPU composition, the applied atomic enable on real
    /// hardware, then the same world/server/listener assembly as
    /// headless. [`Compositor::serve_kms`] drives it.
    ///
    /// # Errors
    ///
    /// Every bring-up step fails typed and honest: master takeover
    /// (another compositor owns the device), dumb allocation,
    /// framebuffer registration, mapping, the applied enable commit
    /// (no `TEST_ONLY` to hide behind — the kernel's rejection is
    /// final), plus the shared assembly failures. Reported as
    /// I/O-class [`LdpError`] with context; objects already created
    /// are unwound best-effort on every failure path.
    ///
    /// # Panics
    ///
    /// Only on a poisoned world lock later in the assembly (a session
    /// thread died mid-operation) — startup-fatal by policy.
    // The bring-up is one ordered narrative (device, pipelines,
    // world, sessions); splitting it would hide the unwind ordering.
    #[allow(clippy::too_many_lines)]
    pub fn drm(
        mut backend: DrmBackend,
        config: CompositorConfig,
        quiet: bool,
    ) -> Result<Compositor> {
        let mut config = config;
        // Step 1: the modeset privilege.
        backend.set_master().map_err(|e| {
            LdpError::Io(Arc::new(std::io::Error::other(format!(
                "drmSetMaster failed: {e} (another master owns the device?)"
            ))))
        })?;
        // Step 2: the pipeline shape over the real topology — the
        // sized doctrine when the operator forced one (Phase 30: the
        // exact size or the honest typed failure with the offered
        // menu; never a silent nearest-neighbor guess), the
        // multi-output doctrine when enabled (Phase 31: every
        // pipeline the device can serve), and the mode foundry's
        // pour when armed (Phase 42: every connector's own EDID
        // range limits gate its pour — the ceiling-refusing
        // connector is skipped honestly, nothing serving surfaces
        // the typed refusal).
        let selections: Vec<serve::Pipeline> = if config.multi_output {
            match (config.resolution, config.synth) {
                (_, Some(synth)) => {
                    serve::select_pipelines_synth(&backend, synth).map_err(|e| {
                        LdpError::Io(Arc::new(std::io::Error::other(format!("synth {e}"))))
                    })?
                }
                (Some(size), None) => {
                    serve::select_pipelines(&backend, Some(size)).map_err(|e| {
                        LdpError::Io(Arc::new(std::io::Error::other(format!("resolution {e}"))))
                    })?
                }
                (None, None) => serve::select_pipelines(&backend, None)
                    .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e.to_string()))))?,
            }
        } else {
            vec![match (config.resolution, config.synth) {
                (_, Some(synth)) => serve::select_pipeline_synth(&backend, synth).map_err(|e| {
                    LdpError::Io(Arc::new(std::io::Error::other(format!("synth {e}"))))
                })?,
                (Some(size), None) => {
                    serve::select_pipeline_sized(&backend, Some(size)).map_err(|e| {
                        LdpError::Io(Arc::new(std::io::Error::other(format!("resolution {e}"))))
                    })?
                }
                (None, None) => serve::select_pipeline(&backend).ok_or_else(|| {
                    LdpError::Io(Arc::new(std::io::Error::other(
                        "no connected connector with modes on the DRM device",
                    )))
                })?,
            }]
        };
        // Steps 3-6 per pipeline: the output's protocol face, dumb
        // buffers, framebuffers, mappings, the applied enable (the
        // allocation unwinds its own partial state on failure).
        let mut slots: Vec<crate::scene::OutputSlot> = Vec::with_capacity(selections.len());
        for selection in &selections {
            let mut output =
                OutputGlobal::from_backend(&backend, selection, (0, 0)).ok_or_else(|| {
                    LdpError::Io(Arc::new(std::io::Error::other(
                        "the selected pipeline's connector vanished mid-bring-up",
                    )))
                })?;
            // The protocol face follows the forced mode (the selection
            // already proved the match exists on this connector — the
            // re-point cannot miss; the guard stays honest anyway);
            // a foundry pour is *adopted* instead (Phase 42 — the
            // user-defined mode joins the advertised list).
            if selection.mode.kind.0 & ModeType::USERDEF.0 != 0 {
                output.adopt_mode(&selection.mode);
            } else if let Some(size) = config.resolution {
                if !output.select_size(size) {
                    return Err(LdpError::Io(Arc::new(std::io::Error::other(
                        "the forced resolution vanished mid-bring-up",
                    ))));
                }
            }
            let (w, h) = (selection.width(), selection.height());
            let (handles, fbs, mappings, pitch) = allocate_drm_scanout(&mut backend, w, h)
                .map_err(|e| {
                    let _ = backend.drop_master();
                    LdpError::Io(Arc::new(std::io::Error::other(e.to_string())))
                })?;
            if !quiet {
                println!(
                    "scanout: {w}x{h} XRGB8888 on {} (pitch {pitch}), dual dumb buffers mapped",
                    output.name
                );
                // Phase 42: the EDID audit's honest diagnosis — every
                // named finding with its quirk row's escape, one line
                // each (the bring-up teaches the fix).
                for finding in &output.edid_findings {
                    let escape = ldp_display::quirks::by_name(finding.quirk_name())
                        .map_or("none", |q| q.escape);
                    println!("edid: {finding} — escape: {escape}");
                }
                // The foundry's pour line: when this output serves a
                // user-defined timing, say so (the operator's own ask).
                if output.serves_poured_mode() {
                    let mode = output.mode();
                    println!(
                        "synth: {} CVT-RB {} kHz (userdef) on {}",
                        mode, mode.clock_khz, output.name
                    );
                }
            }
            let scanout = ScanoutChain::mapped(mappings, pitch as usize, w, h, fbs);
            // The applied enable commit — for real.
            serve::enable(&mut backend, selection, fbs[0]).map_err(|e| {
                unwind_drm(&mut backend, handles, fbs);
                display_bringup(&e)
            })?;
            let planes = plane_inventory_for(&backend, selection.crtc);
            let psr_capable = psr_capable_for(&backend, selection.connector);
            slots.push(crate::scene::OutputSlot {
                output,
                crtc: selection.crtc,
                plane: selection.plane,
                scanout,
                lease: Some(DrmLease { handles, fbs }),
                flips: 0,
                pending: ldp_core::geometry::Region::new(),
                owes: false,
                planes,
                active_planes: Vec::new(),
                all_planes: false,
                display: Vec::new(),
                psr: ldp_power::psr::PsrMachine::default_machine(),
                psr_capable,
                psr_live: false,
            });
        }
        if !quiet {
            println!(
                "modeset: {} pipeline(s) enabled (applied atomic commit)",
                slots.len()
            );
        }

        // ---- the world (the real driver) ---------------------------
        let mut world = build_world(
            Box::new(DrmDriver::new(backend)),
            slots,
            config.scheduler,
            config.dump.clone(),
            config.renderer,
            config.renderer_api.take(),
            config.effects,
            config.shell,
            config.resolution,
            config.synth,
            config.scale,
            config.output_scales.clone(),
            config.arrangement,
            config.hdr,
            config.hdr_peak,
            VrrSetup::from_config(&config),
            config.idle_ms,
            config.psr,
            config.multi_output,
            config.input,
            false,
        )?;
        // Phase 47: the compositor-owned choreography switch (the
        // headless doctrine — the same switch, the mock world).
        world.scene.transitions.enabled = config.transitions;
        // Phase 49: the states arm's space count (the headless
        // doctrine — the same switch, the mock world).
        world.spaces.set_count(config.workspaces);
        // The kernel objects this session owns live in the slots'
        // leases (the world keeps them so live re-arrangement — Phase
        // 26/31 — can release and re-mint them as the topology moves).
        // Latch the timeline on real hardware: the enable flips land
        // at the next vblanks — the scheduler's first anchors.
        world.land_bringup_flip();
        if !quiet {
            println!("renderer: {}", world.renderer_report);
            println!("effects: {}", world.effects_decision_report());
            println!("shell: {}", world.shell_decision_report());
            println!("scanout serving: bring-up flip landed (Ctrl-C to exit)");
        }
        finish_assembly(world, &config.socket)
    }

    /// Accept one connection and spawn its session thread.
    ///
    /// # Errors
    ///
    /// Accept or session-construction failures (the socket closes).
    pub fn accept_one(&mut self) -> Result<()> {
        // Over max_clients: already refused (the socket closed).
        let Some(session) = self.server.accept_session(&mut self.listener)? else {
            return Ok(());
        };
        let shared = Arc::clone(&self.shared);
        self.server.spawn_session(session, move || {
            CompositorDispatcher::new(Arc::clone(&shared))
        })
    }

    /// The blocking accept loop (the headless binary's `serve` mode).
    ///
    /// # Errors
    ///
    /// Propagates accept failures (the loop's only exit).
    pub fn serve_blocking(&mut self) -> Result<()> {
        loop {
            self.accept_one()?;
        }
    }

    /// The KMS serve loop: poll the listening socket *and* the DRM fd
    /// until shutdown, then tear the pipeline down. The DRM fd wakes
    /// on device events (flips orphaned by finished sessions,
    /// topology changes); the socket wakes on clients; the periodic
    /// timeout re-checks the shutdown flag (Ctrl-C, SIGTERM).
    ///
    /// # Errors
    ///
    /// Accept failures and device-event failures — fatal, honestly
    /// reported (display pipelines do not degrade silently). Teardown
    /// runs on every exit path regardless.
    ///
    /// # Panics
    ///
    /// On a poisoned world lock (a session thread died mid-operation)
    /// — serve-fatal by policy.
    pub fn serve_kms(&mut self, quiet: bool) -> Result<()> {
        // The DRM fd lives inside the world's driver; ask it once.
        let real_fd = {
            let world = self.shared.world.lock().expect("world lock");
            world.drm_fd()
        };
        let listener_fd = self.listener.raw_fd();
        loop {
            if sys::shutdown_requested() {
                break;
            }
            // One poll turn over the descriptors: up to a
            // quarter-second on the socket (the shutdown-latency cap),
            // then immediate checks of the DRM fd and the input
            // devices — events that arrived during the socket wait are
            // caught here.
            let socket_ready = ldp_display::drm::sys::poll_readable(listener_fd, 250)
                .map_err(|e| display_io(&e))?;
            let drm_ready = real_fd >= 0
                && ldp_display::drm::sys::poll_readable(real_fd, 0).map_err(|e| display_io(&e))?;
            if socket_ready {
                self.accept_one()?;
            }
            // The evdev devices (Phase 33): each readable fd pumps
            // through the router and parks the routed events in the
            // outboxes (the clients drain them at their wake points).
            {
                let mut world = self.shared.world.lock().expect("world lock");
                let interests = world.input.interests();
                let ready = interests
                    .iter()
                    .filter(|fd| ldp_display::drm::sys::poll_readable(**fd, 0).unwrap_or(false))
                    .copied()
                    .collect::<Vec<_>>();
                if !ready.is_empty() {
                    let routed = world.pump_input();
                    let _ = routed;
                }
            }
            // The power cadence (Phase 35): the idle ladder ticks on
            // EVERY loop turn now — the poll timeout (≤ 250 ms) is the
            // pace-maker. The Phase 31 wiring ticked only under
            // `drm_ready`, which is exactly backwards: a *truly* idle
            // machine has no DRM events (no flips, no hotplug), so the
            // ladder that exists to detect inactivity never advanced
            // when the machine was actually inactive — a real-machine
            // bug the mock tests could not see (their drivers advance
            // the clock per event). The PSR/governor/ledger tick
            // shares the cadence: the static-frame power path.
            {
                let mut world = self.shared.world.lock().expect("world lock");
                if let Err(e) = world.tick_idle() {
                    eprintln!("lion-compositor: idle tick failed: {e}");
                }
                if let Err(e) = world.psr_tick() {
                    eprintln!("lion-compositor: power tick failed: {e}");
                }
                // Phase 47: compositor-owned motion is server-driven —
                // while a transition is live, the poll cadence (the
                // same ≤ 250 ms pace-maker the power ladder rides)
                // wakes the world through a full pump under the system
                // identity (every emission parks in its owner's
                // outbox; the fade advances, the frame renders, the
                // desktop's own clients never need to be awake for
                // the desktop's own choreography).
                if let Err(e) = world.pump_animations() {
                    eprintln!("lion-compositor: animation tick failed: {e}");
                }
            }
            if drm_ready {
                let mut world = self.shared.world.lock().expect("world lock");
                match world.service_device_events() {
                    Err(e) => {
                        drop(world);
                        eprintln!("lion-compositor: device service failed: {e}");
                        let _ = self.teardown(quiet);
                        return Err(LdpError::Io(Arc::new(std::io::Error::other(e.to_string()))));
                    }
                    Ok(Some(summary)) if !quiet => {
                        report_rearrange(&summary);
                    }
                    Ok(_) => {}
                }
            }
        }
        if !quiet {
            println!("shutdown: tearing the pipeline down");
        }
        self.teardown(quiet)
    }

    /// Teardown: disable the pipeline (the applied mirror commit),
    /// then — on the world's DRM lease — release the framebuffers,
    /// destroy the dumb buffers, and drop master. The mock path
    /// proves the disable in CI; the real path leaves the display
    /// dark, not wedged. A dark world has nothing left to release
    /// (the re-arrangement already did) — the honest no-op.
    ///
    /// # Errors
    ///
    /// The first failure surfaces (reported); the remaining steps run
    /// best-effort — teardown is an exit path, never a retry loop.
    ///
    /// # Panics
    ///
    /// On a poisoned world lock — exit-fatal by policy.
    pub fn teardown(&mut self, quiet: bool) -> Result<()> {
        let mut first: Option<String> = None;
        let leases = {
            let mut world = self.shared.world.lock().expect("world lock");
            // The hardware arm's session summary (Phase 34): the
            // zero-composite frames and imported buffers the operator
            // reads at shutdown.
            if !quiet {
                println!("planes: {}", crate::plane_session::session_report(&world));
            }
            // The power session's summary (Phase 35): the energy
            // ledger's verdict — the static-scene ratio, the
            // self-refresh seconds, the exits and wakes. The line the
            // comparison table's power row is measured from.
            if !quiet {
                println!("power: {}", world.power_report());
            }
            if let Err(e) = world.teardown() {
                first = Some(format!("disable commit rejected: {e}"));
            }
            world.take_leases()
        };
        // The world's driver owns the backend; borrow it back for the
        // object-release vocabulary (one master per device, dropped
        // once, after the last lease's objects).
        let mut world = self.shared.world.lock().expect("world lock");
        if let Some(backend) = world.drm_backend_mut() {
            for DrmLease { handles, fbs } in &leases {
                for fb in fbs {
                    if let Err(e) = backend.rm_fb(*fb) {
                        first = first.or_else(|| Some(format!("drmModeRmFB: {e}")));
                    }
                }
                for handle in handles {
                    if let Err(e) = backend.destroy_dumb(*handle) {
                        first = first.or_else(|| Some(format!("DESTROY_DUMB: {e}")));
                    }
                }
            }
            if !leases.is_empty() {
                if let Err(e) = backend.drop_master() {
                    first = first.or_else(|| Some(format!("drmDropMaster: {e}")));
                }
            }
        }
        if !quiet {
            println!("teardown: pipeline disabled, objects released, master dropped");
        }
        match first {
            Some(why) => Err(LdpError::Io(Arc::new(std::io::Error::other(why)))),
            None => Ok(()),
        }
    }
}

/// The serve loop's re-arrangement report line — what the operator
/// sees when the topology moves under a serving compositor.
fn report_rearrange(summary: &crate::rearrange::Rearrange) {
    use crate::rearrange::Rearrange;
    match summary {
        Rearrange::Spurious => {
            println!("hotplug: topology changed; the served pipeline holds");
        }
        Rearrange::Migrated { from, to } => {
            println!(
                "hotplug: migrated {} ({}x{}) -> {} ({}x{})",
                from.name, from.width, from.height, to.name, to.width, to.height
            );
        }
        Rearrange::Dark { from } => {
            println!(
                "hotplug: {} gone — dark (protocol serving; waiting for a display)",
                from.name
            );
        }
        Rearrange::Relit { to } => {
            println!("hotplug: {} lit — the desktop paints again", to.name);
        }
        Rearrange::OutputAdded { to } => {
            println!(
                "hotplug: {} joined — the desktop extends to {}x{}",
                to.name, to.width, to.height
            );
        }
        Rearrange::OutputRemoved { from } => {
            println!(
                "hotplug: {} left — the desktop re-flows onto the survivors",
                from.name
            );
        }
        Rearrange::Rewired {
            added,
            removed,
            migrated,
        } => {
            let names = |v: &[crate::rearrange::Served]| {
                v.iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            println!(
                "hotplug: rewired (+{} [{}], -{} [{}], ~{} [{}])",
                added.len(),
                names(added),
                removed.len(),
                names(removed),
                migrated.len(),
                migrated
                    .iter()
                    .map(|(f, t)| format!("{}->{}", f.name, t.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
}

/// Assemble the world (shared by both bring-up paths).
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]
fn build_world(
    device: Box<dyn DisplayDriver>,
    slots: Vec<crate::scene::OutputSlot>,
    scheduler: SchedulerConfig,
    dump: Option<std::path::PathBuf>,
    renderer: ldp_renderer::gles::RendererChoice,
    renderer_api: Option<Box<dyn ldp_renderer::gles::api::GlesApi>>,
    effects: ldp_renderer::EffectChoice,
    shell_config: crate::shell::ShellConfig,
    resolution: Option<ModeSize>,
    synth: Option<ldp_display::timing::SynthRequest>,
    scale: ldp_core::scale::ScaleFactor,
    output_scales: Vec<ldp_core::scale::ScaleFactor>,
    arrangement: crate::scene::OutputArrangement,
    hdr: bool,
    hdr_peak: Option<u32>,
    vrr_setup: VrrSetup,
    idle_ms: Option<u32>,
    psr: bool,
    multi_output: bool,
    input_mode: crate::input::InputMode,
    headless: bool,
) -> Result<World> {
    // Phase 41's quirk ledger — the floor pass, BEFORE any consumer:
    // every output's advertised window clamps to the operator's
    // honest floor (the `vrr-floor-flicker` quirk class — the
    // advertisement is the bloated side, the panel's own behavior
    // the truth), so the scheduler's widening, the `output.vrr`
    // event clients see, and the LFC cadence all read the effective
    // window. The pass runs whether or not VRR is armed — the
    // operator's truth about the panel is the truth regardless —
    // and fails boot on an unservable floor (the honest knob:
    // better a typed refusal than a silently ignored lie).
    let mut slots = slots;
    let mut vrr_floor_outcomes = Vec::with_capacity(slots.len());
    for (index, slot) in slots.iter_mut().enumerate() {
        let floor_hz = vrr_setup.floor_for(index);
        if floor_hz == 0 {
            vrr_floor_outcomes.push(ldp_vrr::FloorOutcome::Passthrough);
            continue;
        }
        let nominal_ns = slot.output.refresh().as_ns();
        let (effective, outcome) =
            ldp_vrr::quirk::apply_floor(slot.output.vrr, floor_hz, nominal_ns).map_err(|e| {
                LdpError::Io(Arc::new(std::io::Error::other(format!(
                    "--vrr-floor {floor_hz}: {e}"
                ))))
            })?;
        slot.output.vrr = effective;
        vrr_floor_outcomes.push(outcome);
    }
    // Phase 41's uniform collapse (the `vrr-sibling-flicker` escape):
    // a mixed desktop under `--vrr-uniform` serves one fixed sync
    // across the seam — no output arms, no widening. The default is
    // the modern doctrine (per-output VRR); the collapse only bites
    // when the desktop actually spans the seam (at least one window,
    // at least one without).
    let vrr_collapse = vrr_setup.enabled
        && vrr_setup.uniform
        && slots.iter().any(|s| s.output.vrr.is_none())
        && slots.iter().any(|s| s.output.vrr.is_some());
    // The adaptive-sync wiring (Phase 31, `--vrr`) widens the
    // scheduler's commit window to the primary's *effective* range
    // (Phase 41: the floor pass already clamped it; a collapsed
    // desktop widens nothing — one fixed clock).
    let scheduler = widen_scheduler_for_vrr(scheduler, &slots, vrr_setup.enabled && !vrr_collapse);
    // The primary anchors the pacing grid (slot 0 — the allocator's
    // first answer, resource order).
    let primary_refresh = slots.first().map_or_else(
        || ldp_core::time::RefreshInterval::from_millihz(60_000).expect("60 Hz is a positive rate"),
        |slot| slot.output.refresh(),
    );
    let scene = Scene::new(primary_refresh, scheduler).map_err(|_| {
        LdpError::Io(Arc::new(std::io::Error::other(
            "scheduler config is malformed",
        )))
    })?;
    // The renderer selection (hardware first): Auto probes the
    // real EGL+GLES context and takes it when present, falling back
    // to software with the reason in the report line.
    let (renderer, decision) = crate::renderer::build_renderer(renderer, renderer_api)?;
    // The Liquid tier (Phase 27): the machine facts decide — a GL
    // renderer serves the full language, a software renderer scales
    // the blur by the output's pixel budget, and the headless mock
    // path (the CI doctrine) stays plain unless the operator asked.
    let mode = slots
        .first()
        .map_or_else(ldp_display::Mode::panel_1080p60, |slot| {
            slot.output.mode().clone()
        });
    let output_pixels = u64::from(u32::from(mode.hdisplay)) * u64::from(u32::from(mode.vdisplay));
    let hardware_renderer = decision.backend == ldp_renderer::gles::RendererBackend::Gles;
    let tier = effects.resolve(hardware_renderer, output_pixels, headless);
    // The scanout store kind persists in the world so re-arrangements
    // re-allocate the same kind (shadow on the mock default, mappings
    // for the equivalence suites and the DRM path).
    let mapped_scanout = slots.first().is_some_and(|slot| {
        matches!(
            slot.scanout.store,
            crate::scene::ScanoutStore::Mapped { .. }
        )
    });
    // The bring-up's topology snapshot — the "before" the first
    // re-probe diffs against (Phase 26).
    let topology = topology_statuses(device.as_ref());
    // The scene's per-output ledgers: one entry per served CRTC (the
    // release gates queue against them).
    let mut scene = scene;
    for slot in &slots {
        scene.flips.insert(slot.crtc, 0);
    }
    for (index, slot) in slots.iter_mut().enumerate() {
        // Phase 37: the per-output doctrine — output *i* advertises
        // factor *i*, extras reusing the last entry (the stretch
        // rule); an empty list keeps the Phase 31 doctrine: the one
        // factor everywhere.
        slot.output.scale = output_scales
            .get(index)
            .or(output_scales.last())
            .copied()
            .unwrap_or(scale);
        if hdr {
            // The forced HDR pipeline (`--hdr`, Phase 31): the panel
            // advertises PQ with a 600-nit peak and wide gamut — the
            // honest operator-forced caps (a real EDID/HDR-property
            // walk is the DRM-side roadmap line). Phase 38: the peak
            // is the *negotiated* one — `--hdr-peak N` sets the
            // operator's truth (the bloated-peak quirk class: real
            // panels advertise more than they sustain), and the
            // advertisement carries the effective value both ways
            // (what `hdr_caps` tells clients, what the render pass
            // tone-maps against).
            let advertised = ldp_core::color::Luminance::from_nits(600);
            let operator = hdr_peak.map(ldp_core::color::Luminance::from_nits);
            let peak = ldp_hdr::PanelPeak::new(advertised, operator)
                .map_err(|e| {
                    LdpError::Io(Arc::new(std::io::Error::other(format!("--hdr-peak: {e}"))))
                })?
                .effective();
            slot.output.hdr = Some(ldp_hdr::policy::OutputCaps {
                hdr: true,
                pq: true,
                hlg: false,
                max_luminance: peak,
                wide_gamut: true,
            });
        }
    }
    // The output-mode controller (the dwell hysteresis): default SDR.
    let hdr_state =
        hdr.then(|| ldp_hdr::policy::ModeController::new(ldp_hdr::policy::OutputMode::Sdr, 2));
    // The idle ladder (Phase 31, `--idle MS`): the ldp-power machine
    // with the operator's inactivity timeout (Dimmed at the timeout,
    // Off at twice it — the ladder's own spacing); `None` never
    // sleeps.
    let idle_machine = idle_ms.map(|ms| {
        ldp_power::idle::IdleMachine::new(
            ldp_core::time::Mono::ZERO,
            // The timeouts are deltas from the *previous stage's
            // entry* (the logind semantics): dim at MS of
            // inactivity, off MS after the dim (2*MS total), the
            // suspend vocabulary MS after that (3*MS total).
            ldp_power::idle::IdleTimeouts {
                dim_ms: u64::from(ms),
                off_ms: u64::from(ms),
                suspend_ms: u64::from(ms),
            },
        )
    });
    // The positioning shell (Phase 28): resolve the layout for the
    // *primary* output — the class, the usable area, the dock's
    // reservation (and its intro rise, replayed at every fresh
    // pipeline). The logical layout (Phase 31): the primary at the
    // origin, the others appended right in slot order. The logical
    // canvas resolves on the *primary's own* factor (Phase 37: with
    // per-output scales, the primary's entry — not the global
    // default — sizes the desktop's logical geometry).
    let mut shell = crate::shell::Shell::new(shell_config);
    let primary_scale = slots.first().map_or(scale, |slot| slot.output.scale);
    shell.relayout(
        (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
        primary_scale,
        true,
    );
    // The logical layout (Phase 31): the primary at the origin, the
    // others appended right in slot order — *unless* the arrangement
    // is the mirror (Phase 37): every output at the origin, the
    // overlap itself the clone signal, each display showing the same
    // desktop cropped to its own mode. The first mirrored frame
    // paints every display whole (the full-bounds seed) so a display
    // larger than the desktop letterboxes in painted background from
    // its very first flip, and a smaller one carries no stale fb-init
    // edges.
    let mut slots = slots;
    match arrangement {
        crate::scene::OutputArrangement::Extended => {
            let mut x = 0i32;
            for slot in &mut slots {
                slot.output.layout = (x, 0);
                x += i32::try_from(u32::from(slot.output.mode().hdisplay)).unwrap_or(i32::MAX);
            }
        }
        crate::scene::OutputArrangement::Mirrored => {
            for slot in &mut slots {
                slot.output.layout = (0, 0);
                slot.pending.add(slot.output.bounds());
            }
        }
    }
    // The power clocks' t=0: sampled before `device` moves into the
    // world (the ledger's first interval and the governor's first
    // window share the bring-up instant).
    let power_t0 = device.now();
    // Phase 44: a world that comes up dark (no connector lit at
    // bring-up) starts with the output global withdrawn — the registry
    // tells the truth from the first bind, not only after the first
    // rearrangement teaches it.
    let start_dark = slots.is_empty();
    let mut withdrawn_globals = std::collections::HashSet::new();
    if start_dark {
        withdrawn_globals.insert("ldp.core.output".into());
    }
    Ok(World {
        device,
        outputs: slots,
        scene,
        renderer,
        topology,
        mapped_scanout,
        outboxes: crate::outbox::Outboxes::new(),
        live_registries: HashMap::new(),
        bound_globals: HashMap::new(),
        withdrawn_globals,
        frames: 0,
        device_events: 0,
        rearranges: 0,
        dump,
        renderer_report: decision.report,
        effects: tier,
        shell,
        chrome: crate::shell::ChromePass::default(),
        resolution,
        synth,
        multi_output,
        arrangement,
        output_scales,
        hdr: hdr_state,
        negotiated: None,
        vrr_enabled: vrr_setup.enabled,
        vrr_collapse,
        vrr_floor_outcomes,
        idle: idle_machine,
        blanked: false,
        last_input: None,
        input_photon_ns: None,
        psr_enabled: psr,
        governor: ldp_power::governor::ClockGovernor::new(),
        ledger: ldp_power::ledger::EnergyLedger::new(),
        power_sample_at: power_t0,
        governor_at: power_t0,
        governor_flips: 0,
        clipboard: ldp_clipboard::ClipboardManager::new(Limits::default()),
        seat_serial: 0,
        input: crate::input::InputState::bring_up(input_mode, !headless),
        next_data_object: 0,
        imports: HashMap::new(),
        zero_pass_frames: 0,
        popups: crate::shell::PopupHost::new(),
        dialogs: crate::shell::DialogHost::new(),
        toplevels: crate::shell::ToplevelHost::new(),
        drags: crate::shell::DragHost::new(),
        spaces: {
            // Phase 49: the states arm's spaces machine — the default
            // count (the bring-up sites apply the operator's own),
            // the one seat registered on space 0.
            let mut spaces = ldp_shell::Spaces::new(4);
            spaces.add_seat(0, 0);
            spaces
        },
        // Phase 51: the view switch's broadcast set — empty until the
        // first client binds the shell global (every binder joins it
        // there; the switch's `workspace_switched` finds them).
        shell_binds: HashMap::new(),
        // Phase 51: no sweep is running at bring-up (the view
        // switch's own flag — private policy state).
        view_sweep: false,
    })
}

/// The adaptive-sync wiring (Phase 31, `--vrr`): the ldp-vrr
/// deadline policy's decision widens the scheduler's window (the
/// stretch a late commit may consume) when the primary's CRTC
/// carries a window; the fixed nominal grid otherwise (the Phase
/// 25 bytes). Phase 39 adds the window's *min* side: the probed
/// bounds arm the presentation clock's measurement band, so the
/// `presented` refresh reports the cadence the panel actually ran
/// (the LFC fast end no less than the stretched slow end) instead
/// of falling back to the nominal step whenever a landing left the
/// legacy half..1.5x band.
fn widen_scheduler_for_vrr(
    mut scheduler: SchedulerConfig,
    slots: &[crate::scene::OutputSlot],
    vrr: bool,
) -> SchedulerConfig {
    if vrr {
        if let Some(primary) = slots.first() {
            if let Some((min_ns, max_ns)) = primary.output.vrr {
                let nominal_ns = primary.output.refresh().as_ns();
                if max_ns > nominal_ns && min_ns <= nominal_ns {
                    scheduler.vrr_window_ns = max_ns - nominal_ns;
                    scheduler.vrr_min_ns = min_ns;
                }
            }
        }
    }
    scheduler
}

/// Light one mock pipeline as a served slot (the multi-output
/// headless bring-up): the protocol face (with the mock's true VRR
/// window — the same probe `from_mock` uses), the scanout store kind
/// the world was configured with, the applied enable.
fn light_mock_slot(
    device: &mut MockDevice,
    pipeline: &serve::Pipeline,
    resolution: Option<ModeSize>,
    mapped_scanout: bool,
) -> Result<crate::scene::OutputSlot> {
    let mut output = OutputGlobal::from_backend(device, pipeline, (0, 0)).ok_or_else(|| {
        LdpError::Io(Arc::new(std::io::Error::other(
            "the selected pipeline's connector vanished mid-bring-up",
        )))
    })?;
    // The mock's VRR window is the truth the headless cascade always
    // advertised (the real-node property walk stays the documented
    // future item).
    output.vrr = device.vrr_window(pipeline.crtc);
    // Phase 42: the foundry's pour rides the pipeline (a USERDEF
    // mode) — the protocol face adopts it into its advertised list;
    // the sized doctrine keeps its re-point (the two flags are
    // mutually exclusive at the CLI, and the modes they select can
    // never collide).
    if pipeline.mode.kind.0 & ModeType::USERDEF.0 != 0 {
        output.adopt_mode(&pipeline.mode);
    } else if let Some(size) = resolution {
        let _ = output.select_size(size);
    }
    let (w, h) = (pipeline.width(), pipeline.height());
    let pitch = w * 4;
    let scanout = if mapped_scanout {
        let byte_len = usize::try_from(u64::from(pitch) * u64::from(h)).unwrap_or(0);
        let maps = [
            ldp_display::drm::sys::anon_mapping(byte_len).map_err(|e| display_bringup(&e))?,
            ldp_display::drm::sys::anon_mapping(byte_len).map_err(|e| display_bringup(&e))?,
        ];
        let mut fbs = [ldp_display::ids::FbId::new(1).expect("nonzero id"); 2];
        for (i, fb) in fbs.iter_mut().enumerate() {
            let handle = u32::try_from(i + 1).expect("handle fits");
            *fb = device
                .add_fb(
                    &FbSpec::single(w, h, FourCC::XRGB8888, handle, pitch, 0),
                    Some(Modifier::LINEAR),
                )
                .map_err(|e| display_bringup(&e))?;
        }
        ScanoutChain::mapped(maps, pitch as usize, w, h, fbs)
    } else {
        let pixels = usize::try_from(w * h).unwrap_or(0);
        let mut fbs = [ldp_display::ids::FbId::new(1).expect("nonzero id"); 2];
        for (i, fb) in fbs.iter_mut().enumerate() {
            let handle = u32::try_from(i + 1).expect("handle fits");
            *fb = device
                .add_fb(
                    &FbSpec::single(w, h, FourCC::XRGB8888, handle, pitch, 0),
                    Some(Modifier::LINEAR),
                )
                .map_err(|e| display_bringup(&e))?;
        }
        ScanoutChain::shadow(pixels, fbs)
    };
    serve::enable(device, pipeline, scanout.fbs[0]).map_err(|e| display_bringup(&e))?;
    let planes = plane_inventory_for(device, pipeline.crtc);
    let psr_capable = psr_capable_for(device, pipeline.connector);
    Ok(crate::scene::OutputSlot {
        output,
        crtc: pipeline.crtc,
        plane: pipeline.plane,
        scanout,
        lease: None,
        flips: 0,
        pending: ldp_core::geometry::Region::new(),
        owes: false,
        planes,
        active_planes: Vec::new(),
        all_planes: false,
        display: Vec::new(),
        psr: ldp_power::psr::PsrMachine::default_machine(),
        psr_capable,
        psr_live: false,
    })
}

/// The stderr audit sink (operator diagnostics): `LDP_AUDIT=stderr`
/// prints every audit record — session fatals arrive with the wire
/// diagnostic verbatim. The default stays the null sink (the
/// production discipline: auditing is opt-in).
#[derive(Clone, Copy, Debug, Default)]
struct StderrAudit;

impl ldp_server::audit::AuditSink for StderrAudit {
    fn record(&self, record: ldp_server::audit::AuditRecord) {
        eprintln!("lion-compositor audit: {record:?}");
    }
}

/// The shared tail of both bring-ups: the protocol server, globals,
/// the abstract-namespace listener.
fn finish_assembly(world: World, socket: &str) -> Result<Compositor> {
    // ---- the protocol server -----------------------------------
    let audit: Arc<dyn ldp_server::audit::AuditSink> =
        if std::env::var("LDP_AUDIT").as_deref() == Ok("stderr") {
            Arc::new(StderrAudit)
        } else {
            Arc::new(ldp_server::audit::NullAudit)
        };
    let server_config = ServerConfig {
        audit,
        globals: vec![
            GlobalAdvert::new("ldp.core.compositor"),
            GlobalAdvert::new("ldp.core.shm"),
            GlobalAdvert::new("ldp.core.output"),
            // The Phase 22 capture surface: grab → read-only frame
            // snapshots (the relay's whole-file vocabulary).
            GlobalAdvert::new("ldp.capture.capture_manager"),
            // Phase 31's service surfaces for the compatibility door:
            // the seat (device proxies for the input event routing)
            // and the shell (the toplevel lifecycle the bridges
            // translate xdg/X windows onto).
            GlobalAdvert::new("ldp.input.seat"),
            GlobalAdvert::new("ldp.shell.shell"),
            // Phase 32's data family: the clipboard and primary
            // selections — devices, sources, offers, and the pipe
            // transfers the receiver drives. The bridges' foreign
            // clients exchange clipboard content through this door.
            GlobalAdvert::new("ldp.data.data_device_manager"),
        ],
        ..ServerConfig::default()
    };
    let server = Server::new(server_config)?;
    // `UnixAddr::Abstract` already carries the abstract-namespace
    // marker (fill_sockaddr writes the leading NUL), so the bind
    // name is exactly `socket` — no manual NUL prefix. (A Phase 10
    // relic doubled the marker, binding "\0<name>" and hiding every
    // external connector until the Phase 18 tools first dialed in.)
    let addr = UnixAddr::abstract_name(socket.as_bytes())?;
    let listener = TransportListener::bind(&addr, 8)?;
    Ok(Compositor {
        shared: Shared::new(world),
        server,
        listener,
        addr,
    })
}

impl World {
    /// The DRM descriptor when the world drives real hardware (the
    /// serve loop polls it); `-1` on the mock path.
    #[must_use]
    pub fn drm_fd(&self) -> i32 {
        self.device.as_drm().map_or(-1, DrmDriver::fd)
    }

    /// The DRM backend behind the world's driver, when real (the
    /// teardown path's object-release vocabulary).
    pub fn drm_backend_mut(&mut self) -> Option<&mut DrmBackend> {
        self.device.as_drm_mut().map(DrmDriver::backend_mut)
    }
}

/// Release bring-up objects after a failed DRM assembly
/// (best-effort, in reverse order; unmade releases fail harmlessly).
fn unwind_drm(backend: &mut DrmBackend, handles: [u32; 2], fbs: [ldp_display::ids::FbId; 2]) {
    for fb in fbs {
        let _ = backend.rm_fb(fb);
    }
    for handle in handles {
        if handle != 0 {
            let _ = backend.destroy_dumb(handle);
        }
    }
    let _ = backend.drop_master();
}

/// Find the primary plane that can feed `crtc`.
fn primary_plane_for(device: &MockDevice, crtc: ldp_display::ids::CrtcId) -> Option<PlaneId> {
    let top = device.topology().ok()?;
    let index = top.crtc_index(crtc)?;
    top.planes.iter().copied().find(|p| {
        device
            .plane_info(*p)
            .is_ok_and(|info| info.kind == PlaneType::Primary && info.feeds_crtc_index(index))
    })
}

fn display_bringup(e: &ldp_display::DisplayError) -> LdpError {
    LdpError::Io(Arc::new(std::io::Error::other(format!(
        "pipeline bring-up rejected: {e}"
    ))))
}

/// A device-layer failure rendered as the serve loop's I/O arm.
fn display_io(e: &ldp_display::DisplayError) -> LdpError {
    LdpError::Io(Arc::new(std::io::Error::other(format!(
        "device poll failed: {e}"
    ))))
}
