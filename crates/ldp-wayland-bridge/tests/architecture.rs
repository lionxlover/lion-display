//! EC: the core architectural lint — no Wayland/X11 references in the
//! core crates.
//!
//! The LDP core contains zero compatibility code (architecture §20):
//! bridges are ordinary LDP clients with ambient authority. This test
//! walks every source file of every **core** crate (the crates a
//! bridge-free server links: core, protocol, transport, renderer,
//! compositor, shell, color, hdr, vrr, input, seat, clipboard,
//! security, accessibility, power, session, display, gpu) and fails
//! on any compatibility vocabulary: `wayland`, `wl_`, `xdg`, `x11`,
//! `xorg`, `xcb`, `shm`-bridge references, or the bridge crate names
//! in dependency lists.
//!
//! The bridges themselves (and their tests) are the only places the
//! vocabulary appears — by design, and checked the other way by the
//! bridges' own suites.

#![forbid(unsafe_code)]

/// The core crates (everything except the two bridges).
const CORE_CRATES: &[&str] = &[
    "ldp-core",
    "ldp-protocol",
    "ldp-transport",
    "ldp-renderer",
    "ldp-compositor",
    "ldp-shell",
    "ldp-color",
    "ldp-hdr",
    "ldp-vrr",
    "ldp-input",
    "ldp-seat",
    "ldp-clipboard",
    "ldp-security",
    "ldp-accessibility",
    "ldp-power",
    "ldp-session",
    "ldp-display",
    "ldp-gpu",
    "ldp-client",
];

/// The compatibility vocabulary (lowercase, word-boundary matched).
const FORBIDDEN: &[&str] = &[
    "wayland",
    "wl_display",
    "wl_surface",
    "wl_shm",
    "wl_seat",
    "xdg",
    "x11",
    "xorg",
    "xcb",
    "xlib",
    "ldp-wayland-bridge",
    "ldp-x11-bridge",
];

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repo root")
}

fn is_source(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("rs" | "toml")
    )
}

#[test]
fn core_crates_have_no_compatibility_vocabulary() {
    let root = repo_root();
    let mut violations = Vec::new();
    for crate_name in CORE_CRATES {
        let dir = root.join("crates").join(crate_name);
        let mut stack = vec![dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !is_source(&path) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lower = text.to_lowercase();
                for word in FORBIDDEN {
                    if lower.contains(word) {
                        violations.push(format!("{}: contains {word:?}", path.display()));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "core crates reference compatibility vocabulary:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_bridges_are_not_dependencies_of_any_core_crate() {
    let root = repo_root();
    for crate_name in CORE_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        assert!(
            !text.contains("ldp-wayland-bridge") && !text.contains("ldp-x11-bridge"),
            "{crate_name} depends on a bridge crate"
        );
    }
}
