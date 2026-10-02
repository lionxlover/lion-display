//! `lion-compositor` — the Phase 10 vertical-slice binary, grown into
//! the real scanout service.
//!
//! Modes:
//!
//! * `--mode headless`: full protocol service on the mock KMS device
//!   — the CI exit criterion.
//! * `--mode drm`: **the real serve loop** (Phase 25) — DRM-Master
//!   takeover, kernel dumb buffers mapped and composited into, the
//!   applied atomic enable, then the poll loop: LDP clients on the
//!   abstract socket while page flips land on the DRM fd; Ctrl-C
//!   tears down honestly (display dark, objects released, master
//!   dropped). No DRM node = typed failure, never a silent fallback.
//! * `--mode auto` (default): try the DRM serve loop; fall back to
//!   headless when no device opens.
//! * `--probe`: the zero-risk pre-flight — discovery plus the Phase 24
//!   rehearsal (everything under `TEST_ONLY`, nothing applied).
//!
//! Renderer selection (Phase 24, the macOS doctrine):
//! `--renderer auto` (the default) composites on the GPU through the
//! EGL+GLES backend whenever the machine has a GL stack and falls
//! back to the software reference with the reason in the startup
//! report; `--renderer gl` forces the GPU path (a machine without
//! GL fails startup, never a silent CPU fallback); `--renderer
//! software` pins the reference backend.
//!
//! Headless time doctrine: the mock clock advances only at protocol
//! wake points, so the served event stream is deterministic in the
//! message sequence — see the library docs.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use lion_compositor::drm_probe;
use lion_compositor::server::{Compositor, CompositorConfig};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Auto,
    Headless,
    Drm,
}

// The bools are operator switches — each names exactly one CLI flag.
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    mode: Mode,
    socket: Option<String>,
    dump: Option<String>,
    renderer: ldp_renderer::gles::RendererChoice,
    effects: ldp_renderer::EffectChoice,
    dock: lion_compositor::shell::DockMode,
    dock_thickness: u32,
    resolution: Option<ldp_display::serve::ModeSize>,
    /// The mode foundry's pour (`--synth WxH[@HZ][:family]`, Phases
    /// 42 and 46): the VESA timing synthesis — CVT-RB by default,
    /// `:rb2`, `:rb2v`, `:cvt`, or `:gtf` by suffix — mutually
    /// exclusive with `--resolution` (the parse refuses both).
    synth: Option<ldp_display::timing::SynthRequest>,
    outputs: OutputDoctrine,
    scale: ldp_core::scale::ScaleFactor,
    /// The per-output scale factors (`--scale F1,F2,…`, Phase 37):
    /// empty is the lone-factor doctrine.
    scales: Vec<ldp_core::scale::ScaleFactor>,
    hdr: bool,
    /// The operator's panel-peak truth (`--hdr-peak NITS`, Phase 38).
    hdr_peak: Option<u32>,
    vrr: bool,
    /// The mixed-desktop escape (`--vrr-uniform`, Phase 41): one
    /// fixed desktop when any output lacks the VRR window.
    vrr_uniform: bool,
    /// The blanket floor (`--vrr-floor N`, Phase 41): every output's
    /// honest minimum refresh rate.
    vrr_floor: Option<u32>,
    /// The per-output floors (`--vrr-floor F1,F2,…`, Phase 41):
    /// empty is the blanket doctrine.
    vrr_floors: Vec<u32>,
    idle_ms: Option<u32>,
    no_psr: bool,
    transitions: bool,
    /// The spaces count (`--workspaces N`, Phase 49): the states
    /// arm's desktop count.
    workspaces: u32,
    input: lion_compositor::input::InputMode,
    selftest: bool,
    probe: bool,
    quiet: bool,
}

/// The multi-output doctrine (`--outputs`, Phase 31; the mirror
/// arrangement joins in Phase 37).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputDoctrine {
    /// Serve every pipeline the device can serve at once (several
    /// CRTCs, several scanout chains, one logical desktop).
    Multi,
    /// Serve exactly one pipeline (the Phase 25/26 doctrine).
    Single,
    /// Serve every pipeline, every display showing the same desktop
    /// (Phase 37's clone doctrine — the primary's bounds cropped to
    /// each mode).
    Mirror,
}

/// The CLI's defaults (the operator-facing doctrine: the home dock
/// on, every display served, everything else neutral — the *library*
/// defaults stay the legacy-off doctrine for the equivalence corpora).
fn cli_defaults() -> Cli {
    Cli {
        mode: Mode::Auto,
        socket: None,
        dump: None,
        renderer: ldp_renderer::gles::RendererChoice::Auto,
        effects: ldp_renderer::EffectChoice::Auto,
        // The CLI serves the home dock by default (the phone
        // doctrine); the library default stays off (byte-exact
        // legacy pixels).
        dock: lion_compositor::shell::DockMode::Auto,
        dock_thickness: 84,
        resolution: None,
        synth: None,
        // The CLI serves every display the machine has — the desktop
        // extends across monitors; the library default stays single
        // (byte-exact for the equivalence corpora).
        outputs: OutputDoctrine::Multi,
        // The CLI serves the machine's real devices with DRM (the
        // desktop doctrine); the library default stays auto (off
        // headless — byte-exact for the equivalence corpora).
        input: lion_compositor::input::InputMode::Auto,
        scale: ldp_core::scale::ScaleFactor::IDENTITY,
        scales: Vec::new(),
        hdr: false,
        hdr_peak: None,
        vrr: false,
        vrr_uniform: false,
        vrr_floor: None,
        vrr_floors: Vec::new(),
        idle_ms: None,
        no_psr: false,
        transitions: false,
        workspaces: 4,
        selftest: false,
        probe: false,
        quiet: false,
    }
}

// The CLI grammar is one narrative (the usage text's mirror);
// splitting it would hide the doctrine order.
#[allow(clippy::too_many_lines)]
fn parse_args() -> Result<Cli, String> {
    let mut cli = cli_defaults();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => {
                let value = args.next().ok_or("--mode needs a value")?;
                cli.mode = match value.as_str() {
                    "auto" => Mode::Auto,
                    "headless" => Mode::Headless,
                    "drm" => Mode::Drm,
                    _ => return Err(format!("unknown mode '{value}' (auto|headless|drm)")),
                };
            }
            "--socket" => {
                cli.socket = Some(args.next().ok_or("--socket needs a value")?);
            }
            "--dump" => {
                cli.dump = Some(args.next().ok_or("--dump needs a directory")?);
            }
            "--renderer" => {
                let value = args.next().ok_or("--renderer needs a value")?;
                cli.renderer = ldp_renderer::gles::RendererChoice::parse(&value)?;
            }
            "--effects" => {
                let value = args.next().ok_or("--effects needs a value")?;
                cli.effects = ldp_renderer::EffectChoice::parse(&value).ok_or_else(|| {
                    format!("unknown effects tier '{value}' (auto|high|medium|low|minimal)")
                })?;
            }
            "--dock" => {
                let value = args.next().ok_or("--dock needs a value")?;
                cli.dock = lion_compositor::shell::DockMode::parse(&value)
                    .ok_or_else(|| format!("unknown dock mode '{value}' (auto|off)"))?;
            }
            "--dock-thickness" => {
                let value = args.next().ok_or("--dock-thickness needs a value")?;
                cli.dock_thickness = value
                    .parse()
                    .map_err(|_| format!("bad thickness '{value}' (a whole number of pixels)"))?;
            }
            "--resolution" => {
                let value = args.next().ok_or("--resolution needs a value")?;
                cli.resolution = Some(ldp_display::serve::ModeSize::parse(&value)?);
            }
            "--synth" => {
                // Phases 42 + 46 — the mode foundry: pour a VESA
                // timing for the asked size (and refresh, default
                // 60; and family, default reduced-blanking v1).
                let value = args.next().ok_or("--synth needs a value")?;
                cli.synth = Some(ldp_display::timing::SynthRequest::parse(&value)?);
            }
            "--scale" => {
                let value = args.next().ok_or("--scale needs a value")?;
                // The per-output doctrine (Phase 37): a comma list
                // names one factor per output in output order
                // (`1,2` — the eDP at 1x, the HDMI at 2x); a lone
                // factor is the Phase 31 doctrine (every output).
                let factors = parse_scale_list(&value)?;
                if factors.len() == 1 {
                    cli.scale = factors[0];
                } else {
                    cli.scales = factors;
                }
            }
            "--hdr" => cli.hdr = true,
            "--hdr-peak" => {
                let value = args.next().ok_or("--hdr-peak needs a value (nits)")?;
                cli.hdr_peak = Some(parse_hdr_peak(&value)?);
            }
            "--vrr" => cli.vrr = true,
            "--vrr-uniform" => {
                // The mixed-desktop escape (Phase 41, the
                // `vrr-sibling-flicker` quirk's route around): pairs
                // with `--vrr` — arming is meaningless without it.
                cli.vrr_uniform = true;
            }
            "--vrr-floor" => {
                let value = args.next().ok_or("--vrr-floor needs a value (Hz)")?;
                // Phase 41 (the quirk ledger): the operator's honest
                // floor — the same grammar as `--scale` (a lone rate
                // blankets every output; a comma list names one rate
                // per output in output order, `0` the per-output
                // passthrough).
                let floors = ldp_vrr::quirk::parse_floor_list(&value)?;
                if floors.len() == 1 {
                    cli.vrr_floor = Some(floors[0]);
                } else {
                    cli.vrr_floors = floors;
                }
            }
            "--no-psr" => cli.no_psr = true,
            "--transitions" => {
                // Phase 47: the compositor-owned choreography —
                // window-open fades over the spring engine, advanced
                // at the pump's cadence (the serve loop's poll tick
                // wakes a desktop whose clients all sleep).
                cli.transitions = true;
            }
            "--workspaces" => {
                // Phase 49: the states arm's space count — how many
                // desktops the one seat carries (0 promotes to 1,
                // the machine's own doctrine).
                let value = args.next().ok_or("--workspaces needs a value")?;
                cli.workspaces = value
                    .parse()
                    .map_err(|_| format!("bad workspace count '{value}'"))?;
            }
            "--idle" => {
                let value = args.next().ok_or("--idle needs a value")?;
                cli.idle_ms = Some(
                    value
                        .parse()
                        .map_err(|_| format!("bad idle timeout '{value}' (milliseconds)"))?,
                );
            }
            "--input" => {
                let value = args.next().ok_or("--input needs a value")?;
                cli.input = lion_compositor::input::InputMode::parse(&value)?;
            }
            "--outputs" => {
                let value = args.next().ok_or("--outputs needs a value")?;
                cli.outputs = match value.as_str() {
                    "multi" | "auto" => OutputDoctrine::Multi,
                    "single" => OutputDoctrine::Single,
                    "mirror" => OutputDoctrine::Mirror,
                    _ => {
                        return Err(format!(
                            "unknown outputs doctrine '{value}' (multi|single|mirror)"
                        ))
                    }
                };
            }
            "--selftest" => cli.selftest = true,
            "--probe" => cli.probe = true,
            "--quiet" => cli.quiet = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("lion-compositor {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}' (try --help)")),
        }
    }
    // Phase 42: the foundry and the sized doctrine are two different
    // asks (a poured timing vs the firmware's offered list) — serving
    // both would need a tiebreak nobody wants. Refuse the pair.
    if cli.resolution.is_some() && cli.synth.is_some() {
        return Err(
            "--resolution and --synth are mutually exclusive (the offered list vs the pour)"
                .to_owned(),
        );
    }
    Ok(cli)
}

/// Parse `--scale`'s value: one or more comma-separated factors,
/// every one a positive real the Q8.8 ladder can carry. The stretch
/// rule (extras reusing the last factor) is the *doctrine's*
/// business — the parser stays dumb and returns exactly what the
/// operator wrote.
fn parse_scale_list(value: &str) -> Result<Vec<ldp_core::scale::ScaleFactor>, String> {
    let mut factors = Vec::new();
    for part in value.split(',') {
        let parsed: f32 = part
            .trim()
            .parse()
            .map_err(|_| format!("bad scale '{part}' (a positive factor like 1.25)"))?;
        let factor = ldp_core::scale::ScaleFactor::from_f32_lossy(parsed)
            .ok_or_else(|| format!("bad scale '{part}' (a positive factor like 1.25)"))?;
        factors.push(factor);
    }
    if factors.is_empty() {
        return Err("--scale needs at least one factor".to_owned());
    }
    Ok(factors)
}

/// The selftest's report lines — the operator's one-screen truth
/// about what serves: the renderer, the effects tier, the shell,
/// the power doctrine, the output arrangement, the quirk table,
/// the VRR ledger's state, the HDR negotiation, the foundry and
/// the EDID audit (Phase 42), and the pipeline-up line that names
/// the first output's own mode (the pour included — the line never
/// lies about what serves).
fn print_selftest_report(compositor: &Compositor) {
    let (renderer, effects, shell, power, outputs, quirks, vrr, hdr, modes) = compositor
        .shared
        .world
        .lock()
        .map(|w| {
            (
                w.renderer_decision_report().to_owned(),
                w.effects_decision_report(),
                w.shell_decision_report(),
                // The power line (Phase 35): the PSR doctrine the
                // pipeline serves with.
                format!(
                    "psr {} (the panel sleeps when static)",
                    if w.psr_enabled { "on" } else { "off" }
                ),
                // The arrangement line (Phase 37): the multi-output
                // doctrine the desktop serves under.
                match w.arrangement {
                    lion_compositor::scene::OutputArrangement::Extended => {
                        "extended (one desktop, outputs left-to-right)".to_owned()
                    }
                    lion_compositor::scene::OutputArrangement::Mirrored => {
                        "mirrored (one desktop, every display a clone)".to_owned()
                    }
                },
                // The quirk line (Phase 37): the named-quirk table the
                // operator reads — every row with its escape.
                ldp_display::quirks::report(),
                // The VRR line (Phase 41): the quirk ledger's own
                // state — the arming, the floor pass's audit trail,
                // the collapse when it holds.
                w.vrr_quirk_report(),
                // The HDR line (Phase 38): the panel negotiation the
                // desktop serves under — the effective peak,
                // operator-capped.
                match w.outputs.first().and_then(|slot| slot.output.hdr) {
                    Some(caps) => format!(
                        "hdr pq (effective peak {} nits{} — the negotiation ceiling)",
                        caps.max_luminance.as_nits(),
                        if caps.max_luminance.as_nits() < 600 {
                            " — operator-capped"
                        } else {
                            ""
                        }
                    ),
                    None => "off (the honest SDR panel)".to_owned(),
                },
                // The modes line (Phase 42): the foundry and the EDID
                // audit — every pour served named, every finding
                // diagnosed (the default's clean panel states itself
                // honestly).
                {
                    let mut parts = Vec::new();
                    for slot in &w.outputs {
                        if slot.output.serves_poured_mode() {
                            let mode = slot.output.mode();
                            parts.push(format!(
                                "{} pours {} (CVT-RB {} kHz, userdef)",
                                slot.output.name, mode, mode.clock_khz
                            ));
                        }
                        for finding in &slot.output.edid_findings {
                            parts.push(format!("{}: {}", slot.output.name, finding));
                        }
                    }
                    if parts.is_empty() {
                        "the firmware's list, taken as offered (no pours, no findings)".to_owned()
                    } else {
                        parts.join("; ")
                    }
                },
            )
        })
        .unwrap_or_default();
    println!("renderer: {renderer}");
    println!("effects: {effects}");
    println!("shell: {shell}");
    println!("power: {power}");
    println!("outputs: {outputs}");
    println!("quirks: {quirks}");
    println!("vrr: {vrr}");
    println!("hdr: {hdr}");
    println!("modes: {modes}");
    // The pipeline-up line names the first output's own mode.
    let up = compositor
        .shared
        .world
        .lock()
        .ok()
        .and_then(|w| {
            w.outputs.first().map(|slot| {
                let mode = slot.output.mode();
                format!(
                    "{} {}x{}@{}",
                    slot.output.name,
                    mode.hdisplay,
                    mode.vdisplay,
                    mode.refresh_hz()
                )
            })
        })
        .unwrap_or_else(|| "no output".to_owned());
    println!("selftest: pipeline up ({up}, 2 FBs, timeline live)");
}

fn print_help() {
    println!(
        "lion-compositor — the LionOS LDP example compositor and real scanout service\n\
         \n\
         USAGE:\n\
         \x20 lion-compositor [OPTIONS]\n\
         \n\
         OPTIONS:\n\
         \x20 --mode <auto|headless|drm>  how to drive the display (default: auto)\n\
         \x20 --socket <NAME>             abstract-namespace socket name\n\
         \x20 --dump <DIR>                write frame_NNNN.ppm per rendered frame\n\
         \x20 --renderer <auto|gl|software>  GPU compositing by default; software fallback (default: auto)\n\
         \x20 --effects <auto|high|medium|low|minimal>  the Liquid visual tier: rounded corners,\n\
         \x20     shadows, frosted glass — auto scales it to the machine (default: auto)\n\
         \x20 --dock <auto|off>           the positioning shell's system dock at the screen edge\n\
         \x20     — frosted home bar, windows live above it (default: auto)\n\
         \x20 --dock-thickness <PX>        the dock's thickness (default: 84)\n\
         \x20 --resolution <WxH>          force the output's mode to the exact size (the\n\
         \x20     connector's list decides — a miss is the honest failure naming every\n\
         \x20     size it offers; default: the panel's preferred mode)\n\
         \x20 --synth <WxH[@HZ][:family]>  the mode foundry: pour a VESA timing for any size when the\n\
         \x20     panel's own list does not carry it — the family names the standard: rb (the\n\
         \x20     default, CVT reduced blanking v1), rb2 (the 80-pixel deep-color-era blank),\n\
         \x20     rb2v (rb2 with the 1000/1001 video-optimized rate — the 59.94 Hz class),\n\
         \x20     cvt (standard CRT blanking, the GTF duty-cycle machinery), gtf (the 1999\n\
         \x20     formula) — a user-defined mode on the wire, gated by the EDID's declared\n\
         \x20     clock ceiling (the refusal names it, never a silent clamp); the refresh\n\
         \x20     defaults to 60 (the custom-resolution machine; the display-size quirk\n\
         \x20     trio's escape)\n\
         \x20 --no-psr                   panel self-refresh off: the panel keeps scanning a static\n\
         \x20     framebuffer (the escape for panels whose self-refresh flickers; default:\n\
         \x20     the panel sleeps when the content is static — the display engine quiet)\n\
         \x20 --idle <MS>                 the power ladder's inactivity timeout: dimmed at MS, blanked
         \x20     (DPMS + scheduler park) at 2*MS, suspend vocabulary at 4*MS; activity relights
         \x20 --transitions              compositor-owned window choreography (Phase 47): windows
         \x20     fade in over the spring engine at the frame clock — the system-level
         \x20     motion every app gets for free, even the badly designed ones
         \x20 --workspaces <N>            the states arm's space count (Phase 49): how many desktops
         \x20     the one seat carries — set_workspace moves windows between them, the
         \x20     shell bind answers with workspace_count (default: 4; 0 promotes to 1)
         \x20 --vrr                       adaptive sync: VRR-capable outputs' flips arm the CRTC's VRR
         \x20     enablement (the panel stretches inside its window; the scheduler's deadline
         \x20     widens by the ldp-vrr policy; default: the fixed nominal grid)
         \x20 --vrr-floor <N|N,N,…>        the operator's honest minimum refresh rate (Hz): the
         \x20     advertised window clamps to it — the honesty knob for panels whose
         \x20     advertised range flickers at the bottom (the vrr-floor-flicker quirk
         \x20     class; the same doctrine as --hdr-peak: the advertisement is the
         \x20     bloated side); a comma list names one floor per output in output order
         \x20     (57,0 — the eDP floored, the HDMI passthrough; 0 is the passthrough;
         \x20     extras reuse the last; with --vrr; default: the panel's own advertisement)
         \x20 --vrr-uniform                 the mixed-desktop escape: when the desktop spans a VRR
         \x20     panel and a fixed one, one uniform fixed sync serves across the seam —
         \x20     the route around the cross-CRTC clock coupling some platforms show as
         \x20     flicker on the fixed sibling (the vrr-sibling-flicker quirk; with --vrr;
         \x20     default: per-output VRR — the modern per-display doctrine)
         \x20 --input <auto|on|off>       the evdev input path: real /dev/input devices through
         \x20     the seat router onto the wire (auto: on with DRM, off headless;
         \x20     the keymap is libxkbcommon's compiled default, or the honest
         \x20     fallback; the report line names what serves)
         \x20 --hdr                       the forced HDR pipeline: outputs advertise PQ + a 600-nit
         \x20     peak, and HDR content (per-surface set_color/set_hdr_metadata) takes the
         \x20     composite onto the PQ canvas (dwell-hysteresis; default: SDR)
         \x20 --hdr-peak <NITS>            the panel-peak truth: what the panel actually sustains — the
         \x20     honesty knob for panels that advertise more peak than they deliver (the
         \x20     bloated-peak quirk class); the advertisement, the canvas ceiling, and
         \x20     every HDR layer's tone mapping carry the effective peak (with --hdr;
         \x20     default: the pipeline's own 600-nit advertisement)
         \x20 --scale <F|F,F,…>            the output scale factor(s) (fractional HiDPI): a lone factor
         \x20     advertises on every output and the shell lays out on the logical canvas — a
         \x20     1080p panel at 2.0 serves a 960x540 phone-density desktop; a comma list names
         \x20     one factor per output in output order (1,2 — the eDP at 1x, the HDMI at 2x;
         \x20     extras reusing the last; default: 1.0)
         \x20 --outputs <multi|single|mirror>  serve every display the device can drive at once —
         \x20     several CRTCs, one logical desktop that extends and re-flows with the
         \x20     topology; mirror places every display on the same desktop — each showing
         \x20     the same content cropped to its own mode (the clone doctrine) (default: multi)
         \x20 --probe                     discovery + the TEST_ONLY rehearsal, then exit\n\
         \x20 --selftest                  bring the pipeline up, then exit\n\
         \x20 --quiet                     suppress informational output\n\
         \x20 --help | --version\n\
         \n\
         Headless mode serves the full protocol on the deterministic mock\n\
         KMS device (the CI vehicle). DRM mode serves for real: master\n\
         rights, mapped dumb scanout, applied atomic modeset, page flips\n\
         on the DRM fd and LDP clients on the socket — Ctrl-C tears\n\
         down cleanly. Auto tries DRM first and falls back headless.\n\
         --probe validates the whole bring-up under TEST_ONLY (nothing\n\
         applied — the screen never blinks)."
    );
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lion-compositor: {e}");
            return ExitCode::from(2);
        }
    };

    // The zero-risk pre-flight: discovery + the TEST_ONLY rehearsal.
    if cli.probe {
        return match drm_probe::run(cli.quiet) {
            drm_probe::ProbeOutcome::Device { .. } => ExitCode::SUCCESS,
            drm_probe::ProbeOutcome::NoLibrary(why) | drm_probe::ProbeOutcome::NoDevice(why) => {
                eprintln!("lion-compositor: {why}");
                ExitCode::FAILURE
            }
        };
    }

    // The selftest: bring the whole headless pipeline up and drop it —
    // startup validation without serving. The renderer selection's
    // honest report prints first (the one line operators read).
    if cli.selftest {
        return match Compositor::headless(build_config(&cli)) {
            Ok(compositor) => {
                if !cli.quiet {
                    print_selftest_report(&compositor);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("selftest failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    match cli.mode {
        Mode::Drm => serve_drm(&cli),
        Mode::Auto => {
            // Auto probes quietly: a device that opens means the real
            // serve loop; the headless machine falls back.
            match drm_probe::open_first(true) {
                Ok((backend, _driver, _node)) => serve_drm_on(backend, &cli),
                Err(_) => serve_headless(&cli),
            }
        }
        Mode::Headless => serve_headless(&cli),
    }
}

/// The real serve loop over a freshly opened backend: assembly,
/// poll-driven service, teardown — the Phase 25 path.
fn serve_drm(cli: &Cli) -> ExitCode {
    match drm_probe::open_first(cli.quiet) {
        Ok((backend, _driver, _node)) => serve_drm_on(backend, cli),
        Err(why) => {
            eprintln!("lion-compositor: {why}");
            eprintln!("--mode drm is explicit; no silent fallback (try --mode auto)");
            ExitCode::FAILURE
        }
    }
}

fn serve_drm_on(backend: ldp_display::drm::DrmBackend, cli: &Cli) -> ExitCode {
    // Ctrl-C / SIGTERM unwind through teardown.
    if let Err(e) = lion_compositor::sys::install_shutdown_handlers() {
        eprintln!("lion-compositor: cannot install shutdown handlers: {e}");
        return ExitCode::FAILURE;
    }
    let mut compositor = match Compositor::drm(backend, build_config(cli), cli.quiet) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lion-compositor: DRM bring-up failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !cli.quiet {
        let (input, planes) = match compositor.shared.world.lock() {
            Ok(w) => (w.input_report(), w.planes_report()),
            Err(_) => (String::from("input: off"), String::from("planes: off")),
        };
        println!("input: {input}");
        println!("planes: {planes}");
        println!(
            "lion-compositor (drm): listening on {} — Ctrl-C to exit",
            compositor.addr.display_string()
        );
    }
    match compositor.serve_kms(cli.quiet) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lion-compositor: serve loop failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn serve_headless(cli: &Cli) -> ExitCode {
    let config = build_config(cli);
    let mut compositor = match Compositor::headless(config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lion-compositor: headless bring-up failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !cli.quiet {
        let (report, effects, shell, input, planes) = compositor
            .shared
            .world
            .lock()
            .map(|w| {
                (
                    w.renderer_decision_report().to_owned(),
                    w.effects_decision_report(),
                    w.shell_decision_report(),
                    w.input_report(),
                    w.planes_report(),
                )
            })
            .unwrap_or_default();
        println!("renderer: {report}");
        println!("effects: {effects}");
        println!("shell: {shell}");
        println!("input: {input}");
        println!("planes: {planes}");
        println!(
            "lion-compositor (headless): listening on {} — Ctrl-C to exit",
            compositor.addr.display_string()
        );
    }
    match compositor.serve_blocking() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lion-compositor: accept loop failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--hdr-peak N` (Phase 38): the panel-peak truth — a whole number
/// of nits inside the PQ range, validated up front (the negotiation's
/// `PanelPeak` gate is the belt under these braces).
fn parse_hdr_peak(value: &str) -> Result<u32, String> {
    let nits: u32 = value
        .parse()
        .map_err(|_| format!("bad hdr peak '{value}' (a whole number of nits, 1..=10000)"))?;
    if nits == 0 || nits > 10_000 {
        return Err(format!(
            "bad hdr peak '{value}' (a whole number of nits, 1..=10000)"
        ));
    }
    Ok(nits)
}

fn build_config(cli: &Cli) -> CompositorConfig {
    let mut config = CompositorConfig::default();
    if let Some(socket) = &cli.socket {
        config.socket.clone_from(socket);
    }
    if let Some(dump) = &cli.dump {
        let path = std::path::PathBuf::from(dump);
        let _ = std::fs::create_dir_all(&path);
        config.dump = Some(path);
    }
    config.renderer = cli.renderer;
    config.effects = cli.effects;
    config.resolution = cli.resolution;
    config.synth = cli.synth;
    config.multi_output = matches!(cli.outputs, OutputDoctrine::Multi | OutputDoctrine::Mirror);
    config.arrangement = match cli.outputs {
        OutputDoctrine::Mirror => lion_compositor::scene::OutputArrangement::Mirrored,
        _ => lion_compositor::scene::OutputArrangement::Extended,
    };
    config.scale = cli.scale;
    config.output_scales.clone_from(&cli.scales);
    config.hdr = cli.hdr;
    config.hdr_peak = cli.hdr_peak;
    config.vrr = cli.vrr;
    // The quirk ledger's operator knobs (Phase 41): the honest floor
    // and the mixed-desktop escape.
    config.vrr_uniform = cli.vrr_uniform;
    config.vrr_floor = cli.vrr_floor;
    config.vrr_floors.clone_from(&cli.vrr_floors);
    config.idle_ms = cli.idle_ms;
    config.psr = !cli.no_psr;
    config.transitions = cli.transitions;
    config.workspaces = cli.workspaces;
    config.input = cli.input;
    config.shell = lion_compositor::shell::ShellConfig {
        dock: cli.dock,
        dock_thickness: cli.dock_thickness,
    };
    config
}
