//! GPU class classification — pure string logic, no platform calls.
//!
//! The desktop doctrine (Phase 30): the compositor meets *every*
//! machine — a discrete Radeon, an integrated Intel, a VM's virtual
//! GPU, and the no-graphics-card box whose only "GL" is Mesa's CPU
//! rasterizer. The renderer policy needs one honest answer from the
//! probe: **what kind of GPU is behind this GL context?**
//!
//! [`GpuClass`] is that answer, and [`classify`] derives it from the
//! `GL_RENDERER` string the context already reports (`glGetString`,
//! core GLES 2.0 — no extension, no vendor-specific call). The table
//! is deliberately boring, documented, and exhaustively tested; the
//! fallback is [`GpuClass::Unknown`] — never a guess presented as a
//! fact.
//!
//! Why the class matters — the **software-rasterizer rule**: when the
//! "hardware" context turns out to be `llvmpipe` (Mesa's CPU
//! rasterizer), GL is *emulated on the same CPU* our specialized
//! software backend already owns, but through a full GL state machine,
//! shader compiler, and texture pipeline. The specialized backend
//! wins that fight — it blends words directly. So `Auto` prefers the
//! software backend on [`GpuClass::SoftwareRasterizer`], with the
//! reason in the report line, and the no-GPU desktop gets the *fast*
//! path by default. `--renderer gl` still forces the GL path (the
//! operator is the final authority, always).
//!
//! The classification input is exactly one string; the vendor string
//! is *not* consulted because renderer strings carry the same
//! information with fewer ambiguous spellings.

#![forbid(unsafe_code)]

/// What kind of GPU answers behind a live GL context.
///
/// Derived from the renderer string only ([`classify`]); carries no
/// performance numbers, no capability flags — the selection policy
/// in the compositor binary (composing the renderer crate's pinned
/// matrix) makes decisions; this type states facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum GpuClass {
    /// The renderer string said nothing this table understands.
    /// Treated as hardware by the selection policy (the honest
    /// default: an unknown GPU is *a GPU*, not a reason to
    /// degrade).
    #[default]
    Unknown,
    /// A discrete GPU (an AMD Radeon RX, an NVIDIA GeForce, an Intel
    /// Arc) — its own silicon, its own VRAM.
    Discrete,
    /// An integrated GPU on the CPU package (Intel UHD/Iris, AMD
    /// Radeon Vega graphics in an APU, Apple-ish "Graphics" shares).
    Integrated,
    /// A virtualized GPU presented to a VM (virgl, VMware SVGA,
    /// QXL, Hyper-V's virtual device).
    Virtual,
    /// A CPU rasterizer pretending to be GL (llvmpipe, swrast,
    /// softpipe, SwiftShader). No GPU exists — the machine has no
    /// graphics card, or the driver failed to load and Mesa fell
    /// back.
    SoftwareRasterizer,
}

impl GpuClass {
    /// The one-line report spelling (startup lines, selftest).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discrete => "discrete GPU",
            Self::Integrated => "integrated GPU",
            Self::Virtual => "virtual GPU",
            Self::SoftwareRasterizer => "software rasterizer (no GPU)",
            Self::Unknown => "unrecognized GPU",
        }
    }

    /// Whether a real GPU (as opposed to silicon-free GL emulation)
    /// backs the context. `Unknown` counts as hardware — the honest
    /// default above.
    #[must_use]
    pub const fn is_hardware(self) -> bool {
        !matches!(self, Self::SoftwareRasterizer)
    }
}

/// The software-rasterizer marker strings, lowercased, matched by
/// substring. Mesa's CPU drivers and the shim drivers all carry
/// these names; the list is the union of what Mesa 24.x, SwiftShader,
/// and the common container/cloud GL stacks actually report.
const SOFTWARE_MARKERS: &[&str] = &[
    "llvmpipe",
    "swrast",
    "softpipe",
    "swiftshader",
    "software rasterizer",
    "mesa offscreen",
    "osmesa",
    "basic render driver", // Windows RDP's software GL, via ports
];

/// The virtual-GPU marker strings (VM devices).
const VIRTUAL_MARKERS: &[&str] = &[
    "virgl",
    "virtio-gpu",
    "vmware",
    "svga3d",
    "qxl",
    "paravirtual",
    "hyper-v",
    "gallium virtio",
];

/// The discrete-GPU marker strings.
const DISCRETE_MARKERS: &[&str] = &[
    "radeon rx",
    "radeon r5",
    "radeon r7",
    "radeon r9",
    "radv",
    "rx vega",
    "geforce",
    "quadro",
    "nvidia",
    "tesla ",
    "arc a",
    "arc b",
    "firepro",
];

/// The integrated-GPU marker strings.
const INTEGRATED_MARKERS: &[&str] = &[
    "intel",
    "iris",
    "uhd graphics",
    "hd graphics",
    "gma ",
    "radeon graphics",
    "vega 3",
    "vega 6",
    "vega 7",
    "vega 8",
    "vega 10",
    "radeon(tm) graphics",
    "arm mali",
    "adreno",
    "powervr",
    "apple m1",
    "apple m2",
    "apple m3",
];

/// Classify a `GL_RENDERER` string.
///
/// Pure and total: any input (empty, garbage, vendor poetry) maps to
/// a class. Matching is case-insensitive substring; the first
/// matching table wins in the order software → virtual → discrete →
/// integrated → unknown, because a renderer string that says
/// `llvmpipe (DRI-something Radeon)` is *software*, whatever else it
/// also mentions.
#[must_use]
pub fn classify(renderer: &str) -> GpuClass {
    let haystack = renderer.to_lowercase();
    if SOFTWARE_MARKERS.iter().any(|m| haystack.contains(m)) {
        return GpuClass::SoftwareRasterizer;
    }
    if VIRTUAL_MARKERS.iter().any(|m| haystack.contains(m)) {
        return GpuClass::Virtual;
    }
    if DISCRETE_MARKERS
        .iter()
        .any(|m| haystack.contains(m.trim_end()))
    {
        return GpuClass::Discrete;
    }
    if INTEGRATED_MARKERS.iter().any(|m| haystack.contains(m)) {
        return GpuClass::Integrated;
    }
    GpuClass::Unknown
}

/// The queried identity of a live GL context: the three core strings
/// (`glGetString` with VENDOR/RENDERER/VERSION) plus the derived
/// [`GpuClass`].
///
/// Built by the probe ([`crate::gles::RealGles::identity`]) once at
/// bring-up; pure data — every consumer downstream (the selection
/// policy, the startup report, the selftest) reads it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct GlIdentity {
    /// `GL_VENDOR` (the vendor's own name — "AMD", "Intel", …).
    pub vendor: String,
    /// `GL_RENDERER` (the specific device — "llvmpipe (LLVM 17)",
    /// "AMD Radeon RX 7900 XTX", …).
    pub renderer: String,
    /// `GL_VERSION` (the implementation version string).
    pub version: String,
    /// The class derived from the renderer string.
    pub class: GpuClass,
}

impl GlIdentity {
    /// Derive an identity from the three raw strings (tests, the
    /// mock context, the probe). Empty renderer maps to
    /// [`GpuClass::Unknown`].
    #[must_use]
    pub fn from_strings(vendor: &str, renderer: &str, version: &str) -> Self {
        Self {
            vendor: vendor.to_owned(),
            renderer: renderer.to_owned(),
            version: version.to_owned(),
            class: classify(renderer),
        }
    }

    /// The one-line summary the renderer-selection report carries:
    /// the class and the device (truncated to keep startup lines
    /// single-line readable; truncation lands on a char boundary).
    #[must_use]
    pub fn summary(&self) -> String {
        let name = self.renderer.trim();
        if name.is_empty() {
            return self.class.as_str().to_owned();
        }
        if name.len() > 48 {
            // Largest char boundary at or below 48 bytes (renderer
            // strings are ASCII in practice, but never assume).
            let cut = name
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|i| *i <= 48)
                .last()
                .unwrap_or(0);
            format!("{}: {}…", self.class.as_str(), &name[..cut])
        } else {
            format!("{}: {}", self.class.as_str(), name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_rasterizers_are_detected() {
        assert_eq!(
            classify("llvmpipe (LLVM 17.0.6, 256 bits)"),
            GpuClass::SoftwareRasterizer
        );
        assert_eq!(classify("Mesa swrast"), GpuClass::SoftwareRasterizer);
        assert_eq!(
            classify("softpipe (LLVM 17.0.6)"),
            GpuClass::SoftwareRasterizer
        );
        assert_eq!(classify("SwiftShader"), GpuClass::SoftwareRasterizer);
        assert_eq!(
            classify("Software Rasterizer"),
            GpuClass::SoftwareRasterizer
        );
        assert_eq!(classify("OSMesa"), GpuClass::SoftwareRasterizer);
    }

    #[test]
    fn software_wins_over_everything_it_shares_a_string_with() {
        // A CPU driver that also mentions the host card is still CPU.
        assert_eq!(
            classify("llvmpipe (DRI detected Radeon RX 6800)"),
            GpuClass::SoftwareRasterizer
        );
        assert_eq!(
            classify("swrast on VMware SVGA3D; VMware, Inc."),
            GpuClass::SoftwareRasterizer
        );
    }

    #[test]
    fn virtual_gpus_are_detected() {
        assert_eq!(classify("virgl (Virtio-GPU Venus)"), GpuClass::Virtual);
        assert_eq!(classify("SVGA3D; build: 8.17"), GpuClass::Virtual);
        assert_eq!(classify("QXL paravirtual"), GpuClass::Virtual);
        assert_eq!(classify("Gallium virtio-gpu"), GpuClass::Virtual);
    }

    #[test]
    fn discrete_gpus_are_detected() {
        assert_eq!(
            classify("AMD Radeon RX 7900 XTX (radeonsi)"),
            GpuClass::Discrete
        );
        assert_eq!(classify("NVIDIA GeForce RTX 4090"), GpuClass::Discrete);
        assert_eq!(classify("Quadro RTX 8000"), GpuClass::Discrete);
        assert_eq!(classify("Intel Arc A770M"), GpuClass::Discrete);
        assert_eq!(classify("Radeon RX Vega 56"), GpuClass::Discrete);
    }

    #[test]
    fn integrated_gpus_are_detected() {
        assert_eq!(
            classify("Mesa Intel(R) UHD Graphics (CML GT2)"),
            GpuClass::Integrated
        );
        assert_eq!(
            classify("Intel(R) Iris(R) Xe Graphics"),
            GpuClass::Integrated
        );
        assert_eq!(
            classify("AMD Radeon(TM) Graphics (radeonsi renoir)"),
            GpuClass::Integrated
        );
        assert_eq!(classify("ARM Mali-G57"), GpuClass::Integrated);
        assert_eq!(classify("Adreno (TM) 740"), GpuClass::Integrated);
    }

    #[test]
    fn unknown_falls_back_honestly() {
        assert_eq!(classify(""), GpuClass::Unknown);
        assert_eq!(
            classify("Some Future Silicon Nobody Ship Yet"),
            GpuClass::Unknown
        );
        assert_eq!(classify("🦀"), GpuClass::Unknown);
    }

    #[test]
    fn unknown_counts_as_hardware_for_the_policy() {
        // The honest default: an unrecognized GPU is still a GPU —
        // never a silent degrade to software.
        assert!(GpuClass::Unknown.is_hardware());
        assert!(GpuClass::Discrete.is_hardware());
        assert!(GpuClass::Integrated.is_hardware());
        assert!(GpuClass::Virtual.is_hardware());
        assert!(!GpuClass::SoftwareRasterizer.is_hardware());
    }

    #[test]
    fn identity_carries_the_class_and_a_summary() {
        let id = GlIdentity::from_strings(
            "AMD",
            "AMD Radeon RX 7900 XTX (radeonsi)",
            "OpenGL ES 3.2 Mesa 24.1.1",
        );
        assert_eq!(id.class, GpuClass::Discrete);
        assert_eq!(
            id.summary(),
            "discrete GPU: AMD Radeon RX 7900 XTX (radeonsi)"
        );

        let ll = GlIdentity::from_strings("Mesa/X.org", "llvmpipe (LLVM 17.0.6, 256 bits)", "…");
        assert_eq!(ll.class, GpuClass::SoftwareRasterizer);
        assert!(ll
            .summary()
            .starts_with("software rasterizer (no GPU): llvmpipe"));
    }

    #[test]
    fn long_renderer_names_truncate_in_the_summary() {
        let long = "X".repeat(200);
        let id = GlIdentity::from_strings("v", &long, "1");
        let summary = id.summary();
        assert_eq!(
            summary.chars().count(),
            "unrecognized GPU: ".chars().count() + 48 + 1
        );
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn empty_renderer_summarizes_to_the_class_alone() {
        let id = GlIdentity::default();
        assert_eq!(id.class, GpuClass::Unknown);
        assert_eq!(id.summary(), "unrecognized GPU");
    }
}
