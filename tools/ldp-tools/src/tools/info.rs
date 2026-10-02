//! `ldp-info` — protocol, live-session, and display/GPU introspection.
//!
//! ```text
//! ldp-info [--spec-dir DIR]         protocol summary (compiled schema)
//! ldp-info --live [--socket NAME]   live server session report
//! ldp-info --display                DRM/KMS + GPU node report (honest probe)
//! ```
//!
//! The default mode prints the compiled protocol surface — every
//! module, interface, global, and opcode count — straight from
//! [`ldp_protocol::REGISTRY`]. `--live` connects, reports the
//! handshake, cross-checks the advertised globals against the compiled
//! registry, prints the output block (when the server offers
//! `ldp.core.output`), and pulls one introspection payload per
//! interface to confirm the served schema matches the build.
//! `--display` probes real DRM nodes and the GPU catalog and reports
//! exactly what it finds — no mock data, no invention (the Phase 9/10
//! honest-probe doctrine).

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

use ldp_display::backend::KmsBackend as _;
use ldp_protocol::REGISTRY;

use crate::args::{Args, UsageError};
use crate::session::ToolSession;
use crate::socket;

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct InfoArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--live`: connect to a running server.
    pub live: bool,
    /// `--display`: DRM/KMS + GPU probe.
    pub display: bool,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments or missing values.
pub fn parse(mut argv: Args) -> Result<InfoArgs, UsageError> {
    let socket = argv.take_value("--socket")?;
    let live = argv.take_flag("--live");
    let display = argv.take_flag("--display");
    argv.require_empty()?;
    Ok(InfoArgs {
        socket,
        live,
        display,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-info — protocol + session + display introspection\n\
     \n\
     USAGE:\n\
     \x20 ldp-info [--live [--socket NAME]] [--display]\n\
     \n\
     OPTIONS:\n\
     \x20 --live              connect to a running server and report the session\n\
     \x20 --socket NAME       abstract socket name (or LDP_SOCKET; default: none)\n\
     \x20 --display           probe DRM/KMS topology and GPU render nodes\n\
     \x20 --help | --version  this text\n\
     \n\
     With no mode flag: the compiled protocol summary (modules,\n\
     interfaces, globals, opcode counts)."
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; protocol failures print and return 1.
pub fn run(args: &InfoArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    let mut failures = 0;
    if !args.live && !args.display {
        protocol_summary(out);
    }
    if args.live {
        failures += live_report(args, out) as i32;
    }
    if args.display {
        display_report(out);
    }
    Ok(i32::from(failures > 0))
}

/// The compiled protocol surface, module by module.
fn protocol_summary(out: &mut dyn Write) {
    let _ = writeln!(out, "protocol (compiled schema):");
    let mut interfaces = 0;
    let mut globals = 0;
    let mut requests = 0;
    let mut events = 0;
    for module in REGISTRY.modules() {
        let mut module_globals = Vec::new();
        let mut module_requests = 0;
        let mut module_events = 0;
        for iface in module.interfaces {
            interfaces += 1;
            module_requests += iface.requests.len();
            module_events += iface.events.len();
            if iface.global {
                globals += 1;
                module_globals.push(iface.name);
            }
        }
        requests += module_requests;
        events += module_events;
        let _ = writeln!(
            out,
            "  {} v{}-v{}: {} interface(s), {} request(s), {} event(s)",
            module.name,
            module.version_min,
            module.version_max,
            module.interfaces.len(),
            module_requests,
            module_events
        );
        if !module_globals.is_empty() {
            let _ = writeln!(out, "    globals: {}", module_globals.join(", "));
        }
    }
    let _ = writeln!(
        out,
        "  total: {interfaces} interface(s) ({globals} global), \
         {requests} request(s), {events} event(s)"
    );
}

/// The live session report: handshake, globals cross-check, output
/// block, and the served-schema introspection sweep.
#[allow(clippy::too_many_lines)] // one linear report narrative
fn live_report(args: &InfoArgs, out: &mut dyn Write) -> bool {
    let addr = match socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-info: {e}");
            return true;
        }
    };
    let mut session = match ToolSession::connect(&addr).and_then(|s| {
        let mut s = s;
        s.bootstrap()?;
        Ok(s)
    }) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-info: connect {} failed: {e}",
                addr.display_string()
            );
            return true;
        }
    };
    let _ = writeln!(
        out,
        "session: {} (connected + bootstrapped)",
        addr.display_string()
    );

    // Handshake facts.
    if let Some(welcome) = session.welcome() {
        let _ = writeln!(
            out,
            "  welcome: protocol release {}, client id {}, app {}, \
             caps {} bit(s) set, sandbox {}",
            welcome.protocol_release,
            welcome.client_id,
            if welcome.app_id.is_empty() {
                "(unverified)"
            } else {
                &welcome.app_id
            },
            welcome.caps.count(),
            welcome.sandbox
        );
    }
    let limits = session.limits();
    let _ = writeln!(
        out,
        "  limits: {} byte messages, {} fds, {} byte strings, {} rects/region",
        limits.message_bytes, limits.fds_per_message, limits.string_bytes, limits.region_rects
    );

    // Globals vs compiled registry.
    let mut mismatches = 0;
    let _ = writeln!(out, "  advertised globals:");
    for global in session.globals() {
        let compiled = REGISTRY
            .modules()
            .iter()
            .flat_map(|m| m.interfaces.iter())
            .find(|i| i.name == global.interface);
        if let Some(iface) = compiled {
            let version_ok =
                global.version_min >= iface.version_min && global.version_max <= iface.version_max;
            let note = if version_ok {
                format!("v{}-v{}", global.version_min, global.version_max)
            } else {
                mismatches += 1;
                format!(
                    "v{}-v{} OUTSIDE compiled v{}-v{}",
                    global.version_min, global.version_max, iface.version_min, iface.version_max
                )
            };
            let _ = writeln!(out, "    {} ({}, global)", global.interface, note);
        } else {
            mismatches += 1;
            let _ = writeln!(out, "    {} (UNKNOWN to this build)", global.interface);
        }
    }

    // Output block — only when the server offers the interface.
    if session.has_global("ldp.core.output") {
        if let Err(e) = output_block(&mut session, out) {
            let _ = writeln!(out, "  output: report failed: {e}");
            mismatches += 1;
        }
    } else {
        let _ = writeln!(out, "  output: interface not advertised (skipped)");
    }

    // Introspection sweep: the served schema must match the build.
    match session.introspect("") {
        Ok(payloads) => {
            let compiled: Vec<&str> = REGISTRY
                .modules()
                .iter()
                .flat_map(|m| m.interfaces.iter().map(|i| i.name))
                .collect();
            let served: Vec<&str> = payloads.iter().map(|(name, _)| name.as_str()).collect();
            if served == compiled {
                let _ = writeln!(
                    out,
                    "  introspection: {} schema payload(s), matches the compiled registry",
                    payloads.len()
                );
            } else {
                mismatches += 1;
                let _ = writeln!(
                    out,
                    "  introspection: MISMATCH — served {} payload(s), compiled registry has {}",
                    served.len(),
                    compiled.len()
                );
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  introspection: unavailable ({e})");
            mismatches += 1;
        }
    }

    if mismatches > 0 {
        let _ = writeln!(out, "ldp-info: {mismatches} mismatch(es) found");
        return true;
    }
    false
}

/// Bind `ldp.core.output` and decode its event cascade.
fn output_block(session: &mut ToolSession, out: &mut dyn Write) -> crate::error::Result<()> {
    let output = session.bind("ldp.core.output")?;
    // One round-trip drains the immediate cascade (capabilities,
    // geometry, modes, current mode, scale, transform, color, vrr,
    // name — per spec order).
    session.roundtrip()?;
    let output_id = output.id().as_u32();
    let mut name = None;
    let mut geometry = None;
    let mut modes = 0usize;
    let mut current_mode = None;
    let mut vrr = None;
    let mut scale = None;
    for record in session.records() {
        if record.target != output_id {
            continue;
        }
        match record.event.as_str() {
            "name" => name = record.str_arg("name").map(str::to_owned),
            "geometry" => {
                geometry = Some(format!(
                    "+{}+{}, {}×{} mm, subpixel {}",
                    record.i32_arg("x").unwrap_or(0),
                    record.i32_arg("y").unwrap_or(0),
                    record.i32_arg("physical_width").unwrap_or(0),
                    record.i32_arg("physical_height").unwrap_or(0),
                    record.u32_arg("subpixel").unwrap_or(0)
                ));
            }
            "mode" => modes += 1,
            "current_mode" => current_mode = record.u32_arg("id"),
            "scale" => scale = record.u32_arg("scale_q8"),
            "vrr" => {
                vrr = Some(format!(
                    "{}-{} mHz (caps {})",
                    record.u32_arg("min_refresh_millihz").unwrap_or(0),
                    record.u32_arg("max_refresh_millihz").unwrap_or(0),
                    crate::value_fmt::format_bitset(match record.arg("modes") {
                        Some(ldp_core::wire::Value::Bitset(b)) => *b,
                        _ => ldp_core::bitset::Bitset128::EMPTY,
                    })
                ));
            }
            _ => {}
        }
    }
    let _ = writeln!(out, "  output:");
    if let Some(name) = name {
        let _ = writeln!(out, "    name: {name}");
    }
    if let Some(geometry) = geometry {
        let _ = writeln!(out, "    geometry: {geometry}");
    }
    let _ = writeln!(out, "    modes: {modes} offered");
    if let Some(current) = current_mode {
        let _ = writeln!(out, "    current mode id: {current}");
    }
    if let Some(scale) = scale {
        let _ = writeln!(
            out,
            "    scale: {}.{:03}",
            scale / 256,
            (scale % 256) * 1000 / 256
        );
    }
    if let Some(vrr) = vrr {
        let _ = writeln!(out, "    vrr: {vrr}");
    }
    Ok(())
}

/// The DRM/KMS + GPU probe: real nodes or an honest "unavailable".
#[allow(clippy::too_many_lines)] // one linear report narrative
fn display_report(out: &mut dyn Write) {
    let _ = writeln!(out, "display/GPU probe (real nodes only — no mock data):");

    // GPU catalog through the runtime libdrm layer.
    let catalog = match ldp_gpu::drm_sys::LibDrmNodes::open() {
        Ok(lib) => ldp_gpu::NodeCatalog::discover(&lib).ok(),
        Err(_) => None,
    };
    match catalog.as_ref() {
        Some(catalog) if !catalog.devices().is_empty() => {
            let _ = writeln!(out, "  GPU devices:");
            for device in catalog.devices() {
                let _ = writeln!(
                    out,
                    "    {} (render {}, primary {}{}), best render node: {}",
                    device.primary.as_deref().unwrap_or("(no primary node)"),
                    if device.has_render { "yes" } else { "no" },
                    if device.has_primary { "yes" } else { "no" },
                    if device.needs_master() {
                        ", needs DRM-Master"
                    } else {
                        ""
                    },
                    device.best_render().unwrap_or("(none)")
                );
            }
            if let Some(node) = catalog.select_render_node() {
                let _ = writeln!(out, "  selected render node: {}", node.path);
            }
        }
        _ => {
            let _ = writeln!(
                out,
                "  GPU devices: none (libdrm unavailable or no nodes — headless)"
            );
        }
    }

    // KMS topology per primary node.
    let mut opened = 0;
    let cards = catalog
        .as_ref()
        .map(|c| {
            c.devices()
                .iter()
                .filter_map(|d| d.primary.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if cards.is_empty() {
        let _ = writeln!(out, "  KMS: no primary nodes to probe");
    }
    for card in &cards {
        let backend = match ldp_display::drm::DrmBackend::open(Path::new(card)) {
            Ok(backend) => backend,
            Err(e) => {
                let _ = writeln!(out, "  KMS {card}: open failed: {e}");
                continue;
            }
        };
        opened += 1;
        let topology = match backend.topology() {
            Ok(t) => t,
            Err(e) => {
                let _ = writeln!(out, "  KMS {card}: topology failed: {e}");
                continue;
            }
        };
        let _ = writeln!(
            out,
            "  KMS {}: {} connector(s), {} CRTC(s), {} plane(s), \
             max framebuffer {}×{}",
            card,
            topology.connectors.len(),
            topology.crtcs.len(),
            topology.planes.len(),
            if topology.max_width == 0 {
                "unlimited".to_owned()
            } else {
                topology.max_width.to_string()
            },
            if topology.max_height == 0 {
                "unlimited".to_owned()
            } else {
                topology.max_height.to_string()
            }
        );
        for connector in &topology.connectors {
            let info = match backend.connector_info(*connector) {
                Ok(info) => info,
                Err(e) => {
                    let _ = writeln!(out, "    {connector}: info failed: {e}");
                    continue;
                }
            };
            let status = match info.status {
                ldp_display::connector::ConnectorStatus::Connected => "connected",
                ldp_display::connector::ConnectorStatus::Disconnected => "disconnected",
                _ => "unknown",
            };
            let mode_note = info
                .modes
                .first()
                .map(|m| {
                    format!(
                        ", preferred {}×{} @ {}.{:03} Hz",
                        m.hdisplay,
                        m.vdisplay,
                        m.refresh_millihz() / 1000,
                        m.refresh_millihz() % 1000
                    )
                })
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "    {}: {} ({} mode(s){})",
                info.kind.full_name(info.type_index),
                status,
                info.modes.len(),
                mode_note
            );
        }
    }
    if opened == 0 {
        let _ = writeln!(out, "  KMS: no node opened successfully");
    }
}

/// The default spec directory (kept for symmetry with `ldp-validate`).
#[must_use]
pub fn default_spec_dir() -> PathBuf {
    PathBuf::from("spec")
}
