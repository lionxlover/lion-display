//! Direct-scanout eligibility (the scheduler's plane-assignment
//! pre-filter, `docs/architecture.md` §9.4).

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::{ColorDescription, Primaries};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::{geometry_for, pattern_buffer};
use ldp_renderer::{scanout_candidate, BufferView, OutputDesc, ScanoutRejection, SurfaceLayer};

fn fullscreen_layer<'a>(
    data: &'a [u8],
    geometry: &BufferGeometry,
    color: ColorDescription,
    transform: Transform,
    opacity: f32,
    opaque: Region,
) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).unwrap();
    SurfaceLayer::new(
        view,
        Rect::new(0, 0, 8, 8),
        transform,
        color,
        opacity,
        opaque,
    )
}

fn output() -> OutputDesc {
    OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap()
}

fn full_opaque() -> Region {
    Region::from_rect(Rect::new(0, 0, 8, 8))
}

#[test]
fn a_clean_fullscreen_opaque_layer_is_eligible() {
    let (data, geometry) = pattern_buffer(8, 8, FourCC::XRGB8888);
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        1.0,
        full_opaque(),
    );
    let decision = scanout_candidate(&layer, &output());
    assert!(decision.eligible);
    assert_eq!(decision.rejection, None);
}

#[test]
fn every_rejection_reason_is_reported() {
    let (data, geometry) = pattern_buffer(8, 8, FourCC::XRGB8888);

    // Not fullscreen: smaller dest.
    let view = BufferView::new(1, &data, geometry.clone()).unwrap();
    let small = SurfaceLayer::new(
        view,
        Rect::new(0, 0, 4, 8),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        full_opaque(),
    );
    assert_eq!(
        scanout_candidate(&small, &output()).rejection,
        Some(ScanoutRejection::NotFullScreen)
    );

    // Transformed.
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Rot90,
        1.0,
        full_opaque(),
    );
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::Transformed)
    );

    // Translucent.
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        0.5,
        full_opaque(),
    );
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::Translucent)
    );

    // Color mismatch.
    let mut p3 = ColorDescription::srgb_sdr();
    p3.primaries = Primaries::DciP3;
    let layer = fullscreen_layer(&data, &geometry, p3, Transform::Normal, 1.0, full_opaque());
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::ColorMismatch)
    );

    // Format mismatch (ARGB buffer on an XRGB output).
    let (argb, argb_g) = pattern_buffer(8, 8, FourCC::ARGB8888);
    let layer = fullscreen_layer(
        &argb,
        &argb_g,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        1.0,
        full_opaque(),
    );
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::FormatMismatch)
    );

    // Not opaque: a hole in the opaque region.
    let mut opaque = full_opaque();
    opaque.add(Rect::new(3, 3, 2, 2)); // still full — now punch via subtract
    let holed = opaque.subtract(&Region::from_rect(Rect::new(3, 3, 2, 2)));
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        1.0,
        holed,
    );
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::NotOpaque)
    );
}

#[test]
fn opaque_region_covering_dest_in_surface_space_suffices() {
    // The opaque region is surface-space; a fullscreen untransformed layer
    // maps 1:1, so covering the dest rect equals covering the output.
    let (data, geometry) = pattern_buffer(8, 8, FourCC::XRGB8888);
    let mut opaque = Region::from_rect(Rect::new(0, 0, 8, 4));
    opaque.add(Rect::new(0, 4, 8, 4));
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        1.0,
        opaque,
    );
    assert!(scanout_candidate(&layer, &output()).eligible);
}

#[test]
fn a_single_pixel_buffer_cannot_cover() {
    let (data, geometry) = pattern_buffer(1, 1, FourCC::XRGB8888);
    let layer = fullscreen_layer(
        &data,
        &geometry,
        ColorDescription::srgb_sdr(),
        Transform::Normal,
        1.0,
        full_opaque(),
    );
    // dest 8x8 but buffer 1x1: output-sized buffers only on planes.
    assert_eq!(
        scanout_candidate(&layer, &output()).rejection,
        Some(ScanoutRejection::Scaled)
    );
    let _ = geometry_for(1, 1, FourCC::XRGB8888); // silence unused import path
    let _ = Modifier::LINEAR;
    let _ = PlaneLayout {
        offset: 0,
        stride: 4,
    };
}
