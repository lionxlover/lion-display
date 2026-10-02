//! EGL bootstrap and the import path over the object-safe seam.
//!
//! The real backend's honest headless outcome (this sandbox has no
//! `libEGL.so.1`) is asserted alongside the mock's configurable
//! capability gating: the *absence* of an extension is a path worth
//! testing, not an error to hide.

use ldp_core::buffer::{FourCC, Modifier};
use ldp_gpu::dmabuf::{DmaBufDescriptor, DmaBufPlane};
use ldp_gpu::egl::{EglApi, EglError, MockEgl, RealEgl, EXT_DMABUF_IMPORT, EXT_DMABUF_MODIFIERS};
use ldp_renderer::testkit::geometry_for;

fn descriptor() -> DmaBufDescriptor {
    DmaBufDescriptor::new(
        8,
        8,
        FourCC::ARGB8888,
        Modifier::LINEAR,
        &[DmaBufPlane {
            fd: 4,
            offset: 0,
            stride: 32,
        }],
    )
    .expect("valid descriptor")
}

#[test]
fn real_backend_reports_the_honest_headless_outcome() {
    let mut egl = RealEgl::unloaded();
    // This sandbox ships no libEGL: the load itself fails, typed.
    match egl.bootstrap() {
        Err(EglError::LibraryLoad {
            library: "libEGL.so.1",
        }) => {
            // The documented headless outcome.
        }
        Err(other) => panic!("unexpected EGL failure: {other:?}"),
        Ok(caps) => {
            // A machine WITH a GL stack: bootstrap must have proven the
            // extension parse at least.
            assert!(caps.version.is_some());
            assert!(!egl.extensions().is_empty());
        }
    }
}

#[test]
fn mock_bootstrap_gates_on_configured_extensions() {
    // Full stack: the import gate opens.
    let mut full = MockEgl::capable();
    let caps = full.bootstrap().expect("bootstrap");
    assert!(caps.can_import());
    assert!(caps.can_import_modifiers());
    assert!(caps.native_fence_sync);
    assert!(full.supports(EXT_DMABUF_IMPORT));
    assert!(full.supports(EXT_DMABUF_MODIFIERS));

    // Base extension only: modifiers stay closed.
    let mut base = MockEgl::with_extensions(&[EXT_DMABUF_IMPORT]);
    let caps = base.bootstrap().expect("bootstrap");
    assert!(caps.can_import());
    assert!(!caps.can_import_modifiers());

    // Nothing: the gate stays shut.
    let mut no_ext = MockEgl::with_extensions(&[]);
    let caps = no_ext.bootstrap().expect("bootstrap");
    assert!(!caps.can_import());

    // Bootstrap is idempotent.
    let again = full.bootstrap().expect("idempotent");
    assert_eq!(again, full.bootstrap().expect("still idempotent"));
}

#[test]
fn mock_import_requires_the_gate_and_registered_fds() {
    let mut egl = MockEgl::capable();
    egl.bootstrap().unwrap();
    // Bootstrap must run before imports resolve.
    let mut unbooted = MockEgl::capable();
    let err = unbooted.import_image(&descriptor()).unwrap_err();
    assert!(matches!(err, EglError::CallFailed { .. }));

    // An unregistered fd is a typed rejection — like an invalid
    // dma-buf on real hardware.
    let err = egl.import_image(&descriptor()).unwrap_err();
    assert!(
        matches!(err, EglError::CallFailed { while_doing, .. } if while_doing.contains("unregistered"))
    );

    // Register the backing buffer: the import lands.
    let geometry = geometry_for(8, 8, FourCC::ARGB8888);
    let storage = geometry.spanned_bytes() as usize;
    egl.register(4, geometry, std::rc::Rc::new(vec![0u8; storage]));
    let image = egl.import_image(&descriptor()).expect("import lands");
    assert_eq!(egl.descriptor_of(image), Some(&descriptor()));
}

#[test]
fn mock_destroy_is_tombstoned_not_renumbered() {
    let mut egl = MockEgl::capable();
    egl.bootstrap().unwrap();
    let geometry = geometry_for(8, 8, FourCC::ARGB8888);
    let storage = geometry.spanned_bytes() as usize;
    egl.register(4, geometry, std::rc::Rc::new(vec![0u8; storage]));
    let first = egl.import_image(&descriptor()).expect("first");
    let second = egl.import_image(&descriptor()).expect("second");

    // Destroying the first must not renumber the second.
    egl.destroy_image(first).expect("destroy");
    assert_eq!(egl.descriptor_of(second), Some(&descriptor()));
    assert_eq!(egl.descriptor_of(first), None);
    // Double destroy is typed.
    assert!(egl.destroy_image(first).is_err());
    assert!(egl.destroy_image(ldp_gpu::egl::ImageHandle(999)).is_err());
}

#[test]
fn extensionless_mock_rejects_imports() {
    let mut egl = MockEgl::with_extensions(&[EXT_DMABUF_IMPORT]);
    egl.bootstrap().unwrap();
    // Modifiers are always in the attribute list, so their absence
    // closes the import path even with the base extension present.
    let err = egl.import_image(&descriptor()).unwrap_err();
    assert!(
        matches!(err, EglError::CallFailed { while_doing, .. } if while_doing.contains("modifiers"))
    );
}
