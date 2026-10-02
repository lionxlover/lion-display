//! The DRM probe: honest reporting of real display nodes, plus the
//! **hardware bring-up rehearsal** (Phase 24, now the `--probe` mode).
//!
//! Discovery walks `drmGetDevices2` over the dlopen'd libdrm and opens
//! primary nodes through `ldp-display`'s `DrmBackend`. Two consumers:
//!
//! * [`open_first`] — discovery and one open device (with its driver
//!   name), the serve path's entry (`--mode drm`, `--mode auto`). No
//!   display state is touched.
//! * [`run`] — the full probe *plus the rehearsal*: every step the
//!   scanout service would take, exercised under `TEST_ONLY` —
//!   DRM-Master takeover, dumb-buffer allocation, framebuffer
//!   registration, the validated-but-never-applied atomic enable —
//!   then unwound and reported. The operator's zero-risk pre-flight;
//!   the serve loop itself (`--mode drm`) applies for real.

#![forbid(unsafe_code)]

use std::path::Path;

use ldp_display::atomic::AtomicRequest;
use ldp_display::backend::KmsBackend;
use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
use ldp_display::drm::DrmBackend;
use ldp_display::fb::FbSpec;
use ldp_display::serve::select_pipeline;
use ldp_gpu::drm_sys::LibDrmNodes;
use ldp_gpu::node::NodeCatalog;

/// What the probe found.
#[derive(Debug)]
pub enum ProbeOutcome {
    /// libdrm itself is absent — nothing to probe.
    NoLibrary(String),
    /// Nodes exist but none could be opened (permissions/absence).
    NoDevice(String),
    /// A device opened and its topology was walked.
    Device {
        /// The driver name.
        driver: String,
        /// Connector/CRTC/plane counts.
        counts: (usize, usize, usize),
        /// The primary node path that opened.
        node: String,
        /// What the scanout rehearsal proved (Phase 24).
        rehearsal: RehearsalReport,
    },
}

/// The hardware bring-up rehearsal's outcome, step by step.
#[allow(clippy::struct_excessive_bools)] // the rehearsal's step flags ARE its checklist
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RehearsalReport {
    /// DRM-Master rights were acquired (and released afterwards).
    pub master: bool,
    /// Dumb scanout buffers were allocated for the mode's geometry.
    pub dumb_buffers: bool,
    /// Framebuffers were registered from the dumb buffers.
    pub framebuffers: bool,
    /// The kernel validated the full atomic enable commit
    /// (`TEST_ONLY` — never applied).
    pub modeset_validated: bool,
    /// The first failure's detail, when a step failed.
    pub failure: Option<String>,
}

impl RehearsalReport {
    /// The report line (`rehearsal: …`).
    #[must_use]
    pub fn line(&self) -> String {
        match &self.failure {
            Some(why) => format!("rehearsal stopped: {why}"),
            None => {
                format!(
                "rehearsal: scanout bring-up VALIDATED (master, dumb FBs {}, modeset test-only)",
                if self.framebuffers { "registered" } else { "—" }
            )
            }
        }
    }
}

/// Discovery plus the first device that opens — the serve path's
/// entry. Prints the discovery walk unless `quiet`. Returns the
/// opened backend, its driver name, and the node path that opened.
///
/// # Errors
/// The honest machine states as strings: no libdrm, enumeration
/// failure, or no openable node (the headless outcome).
pub fn open_first(quiet: bool) -> Result<(DrmBackend, String, String), String> {
    let lib = match LibDrmNodes::open() {
        Ok(l) => l,
        Err(e) => return Err(format!("libdrm unavailable: {e}")),
    };
    let catalog = match NodeCatalog::discover(&lib) {
        Ok(c) => c,
        Err(e) => return Err(format!("drmGetDevices2 failed: {e}")),
    };
    report(
        quiet,
        &format!("{} GPU device(s) discovered", catalog.devices().len()),
    );
    for device in catalog.devices() {
        let render = device.best_render().unwrap_or("(none)");
        report(
            quiet,
            &format!(
                "  primary={:?} render={render} master-fallback={}",
                device.primary,
                device.needs_master()
            ),
        );
    }
    let primaries: Vec<String> = catalog
        .devices()
        .iter()
        .filter_map(|d| d.primary.clone())
        .collect();
    for path in primaries {
        match DrmBackend::open(Path::new(&path)) {
            Ok(backend) => {
                let version = backend.version().map_or(String::new(), |v| v.name);
                let top = backend.topology().map_or((0, 0, 0), |t| {
                    (t.connectors.len(), t.crtcs.len(), t.planes.len())
                });
                report(
                    quiet,
                    &format!(
                        "opened {path}: driver '{version}', {} connectors, {} CRTCs, {} planes",
                        top.0, top.1, top.2
                    ),
                );
                return Ok((backend, version, path));
            }
            Err(e) => report(quiet, &format!("  {path}: open failed ({e})")),
        }
    }
    Err("no DRM device could be opened".into())
}

/// Run the full probe with the rehearsal and print a human report.
#[must_use]
pub fn run(quiet: bool) -> ProbeOutcome {
    let (mut backend, version, node) = match open_first(quiet) {
        Ok(triple) => triple,
        Err(e) => {
            // Split the error back into its honest arms for the
            // outcome type.
            if e.starts_with("libdrm") {
                return ProbeOutcome::NoLibrary(e);
            }
            return ProbeOutcome::NoDevice(e);
        }
    };
    let top = backend.topology().map_or((0, 0, 0), |t| {
        (t.connectors.len(), t.crtcs.len(), t.planes.len())
    });
    let rehearsal = rehearse(&mut backend, quiet);
    report(quiet, &rehearsal.line());
    ProbeOutcome::Device {
        driver: version,
        counts: top,
        node,
        rehearsal,
    }
}

/// The hardware bring-up rehearsal: every step of the real scanout
/// bring-up, validated, then unwound. See the module docs for what it
/// deliberately is not.
fn rehearse(backend: &mut DrmBackend, quiet: bool) -> RehearsalReport {
    let _ = quiet;
    let mut report = RehearsalReport {
        master: false,
        dumb_buffers: false,
        framebuffers: false,
        modeset_validated: false,
        failure: None,
    };
    // The pipeline shape (the shared selection): a connected connector
    // with a preferred (or first) mode, its CRTC, and the primary
    // plane feeding it.
    let Some(shape) = select_pipeline(backend) else {
        report.failure = Some("no connected connector with modes".into());
        return report;
    };
    // Step 1: DRM-Master (the modeset privilege).
    if let Err(e) = backend.set_master() {
        report.failure = Some(format!(
            "drmSetMaster: {e} (another master owns the device?)"
        ));
        return report;
    }
    report.master = true;
    // Unwind the master right on every exit below.
    let outcome = rehearse_with_master(backend, &shape, &mut report);
    let _ = backend.drop_master();
    let _ = outcome;
    report
}

/// The steps that run while holding master rights.
fn rehearse_with_master(
    backend: &mut DrmBackend,
    shape: &ldp_display::serve::Pipeline,
    report: &mut RehearsalReport,
) -> Result<(), String> {
    let (w, h) = (shape.width(), shape.height());
    let pitch = w * 4;
    // Step 2: two dumb scanout buffers for the mode's geometry.
    let mut handles: Vec<u32> = Vec::new();
    for _ in 0..2 {
        match backend.create_dumb(w, h, 32) {
            Ok((handle, got_pitch)) => {
                if got_pitch < pitch {
                    let _ = backend.destroy_dumb(handle);
                    return Err(format!(
                        "dumb pitch {got_pitch} underflows XRGB8888 at {w} wide"
                    ));
                }
                handles.push(handle);
            }
            Err(e) => return Err(format!("CREATE_DUMB at {w}x{h}: {e}")),
        }
    }
    report.dumb_buffers = true;
    // Step 3: framebuffer registration from the dumb handles.
    let mut fbs = Vec::new();
    for handle in &handles {
        match backend.add_fb(
            &FbSpec::single(w, h, ldp_core::buffer::FourCC::XRGB8888, *handle, pitch, 0),
            Some(ldp_core::buffer::Modifier::LINEAR),
        ) {
            Ok(fb) => fbs.push(fb),
            Err(e) => {
                unwind(backend, &handles, &fbs);
                return Err(format!("drmModeAddFB2: {e}"));
            }
        }
    }
    report.framebuffers = true;
    // Step 4: the full atomic enable commit, TEST_ONLY — the kernel
    // validates the modeset without applying it (the user's screen
    // never blinks). The same request shape the serve loop submits
    // for real, over the rehearsal's own objects.
    let request = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .flag(CommitFlags::TEST_ONLY)
        .connector_bind(shape.connector, shape.crtc)
        .crtc_active(shape.crtc, true)
        .crtc_mode(shape.crtc, &shape.mode)
        .plane_on(
            shape.plane,
            shape.crtc,
            fbs[0],
            SrcRect::new(0, 0, w, h).map_err(|e| e.to_string())?,
            DstRect::new(0, 0, w, h).map_err(|e| e.to_string())?,
        );
    match backend.commit(&request) {
        Ok(outcome) if !outcome.applied => {
            report.modeset_validated = true;
        }
        Ok(_) => {
            unwind(backend, &handles, &fbs);
            return Err("TEST_ONLY commit unexpectedly applied".into());
        }
        Err(e) => {
            unwind(backend, &handles, &fbs);
            return Err(format!("TEST_ONLY enable commit rejected: {e}"));
        }
    }
    // Unwind: FBs, then the dumb buffers themselves.
    unwind(backend, &handles, &fbs);
    Ok(())
}

/// Release the rehearsal's objects (best-effort, in reverse order).
fn unwind(backend: &mut DrmBackend, handles: &[u32], fbs: &[ldp_display::ids::FbId]) {
    for fb in fbs {
        let _ = backend.rm_fb(*fb);
    }
    for handle in handles {
        let _ = backend.destroy_dumb(*handle);
    }
}

fn report(quiet: bool, line: &str) {
    if !quiet {
        println!("{line}");
    }
}
