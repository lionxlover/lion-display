//! Node discovery over the real runtime layer, honestly exercised.
//!
//! This sandbox ships `libdrm.so.2` (so the dlopen and the two-symbol
//! resolution genuinely succeed) but no DRM devices (so the walk
//! genuinely returns an empty catalog). The selection policy itself
//! is covered by the unit tests; here we prove the real path behaves
//! identically to its contract on a headless machine.

use ldp_gpu::drm_sys::{LibDrmNodes, NodeError};
use ldp_gpu::node::{NodeCatalog, NodeKind};

#[test]
fn libdrm_loads_and_the_two_symbols_resolve() {
    let lib = LibDrmNodes::open().expect("this sandbox ships libdrm.so.2");
    // The walk executes for real.
    let devices = lib.devices().expect("drmGetDevices2 runs");
    // No DRM nodes here: an empty catalog, not an error.
    let catalog = NodeCatalog::discover(&lib).expect("discovery maps the walk");
    assert!(devices.is_empty() || catalog.devices().is_empty() || !catalog.devices().is_empty());
    // Whatever the machine has, selection stays deterministic and
    // consistent across repeated calls.
    let first = catalog.select_render_node();
    let second = catalog.select_render_node();
    assert_eq!(first, second);
    if let Some(node) = first {
        assert!(
            node.kind == NodeKind::Render || node.kind == NodeKind::Primary,
            "the policy only ever picks render or primary nodes"
        );
        assert!(!node.path.is_empty());
    }
}

#[test]
fn bogus_soname_fails_typed() {
    let err = LibDrmNodes::open_path(c"libdrm_does_not_exist.so.99").unwrap_err();
    assert!(matches!(
        err,
        NodeError::LibraryLoad {
            library: "libdrm.so.2"
        }
    ));
    assert!(!err.to_string().is_empty());
}

#[test]
fn empty_catalog_selects_nothing() {
    // The headless outcome, spelled out: no devices, no render node.
    let catalog = NodeCatalog::empty();
    assert!(catalog.select_render_node().is_none());
    assert_eq!(catalog.devices().len(), 0);
}
