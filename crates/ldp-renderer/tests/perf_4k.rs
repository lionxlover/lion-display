//! The Phase 8 performance EC: 4K single-surface composition under 8 ms in
//! a release build on the 2-core CI box (`docs/roadmap.md`).
//!
//! Debug builds are excluded (the EC names release); the measurement is a
//! median of repeated full-frame composites of a 3840x2160 XRGB pattern
//! through the word-copy path, with warmup so page faults and allocator
//! settling do not count. Correctness of the fast path is spot-checked so
//! the timed work cannot degrade into a no-op.

#![cfg(not(debug_assertions))]

use std::time::Instant;

use ldp_core::buffer::FourCC;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::{full_damage, pattern_buffer, pattern_rgba};
use ldp_renderer::{BufferView, OutputDesc, Renderer, SoftwareRenderer, SurfaceLayer};

const W: u32 = 3840;
const H: u32 = 2160;

fn frame() -> (Vec<u8>, ldp_core::buffer::BufferGeometry) {
    pattern_buffer(W, H, FourCC::XRGB8888)
}

#[test]
fn k4_single_surface_composites_under_8ms() {
    const RUNS: usize = 7;
    let (data, geometry) = frame();
    let output = OutputDesc::new(W, H, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let view = BufferView::new(1, &data, geometry.clone()).unwrap();
    let layer = SurfaceLayer::new(
        view,
        Rect::new(0, 0, W, H),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, W, H)),
    );
    let damage = full_damage(&output);

    let mut renderer = SoftwareRenderer::new();
    // Warmup: two full frames (page-in, allocator, icache).
    for _ in 0..2 {
        renderer.begin_frame(&output, &damage).unwrap();
        renderer.submit(std::slice::from_ref(&layer)).unwrap();
        renderer.end_frame().unwrap();
    }

    let mut samples = [0f64; RUNS];
    for (i, slot) in samples.iter_mut().enumerate() {
        renderer.begin_frame(&output, &damage).unwrap();
        let t0 = Instant::now();
        let stats = renderer.submit(std::slice::from_ref(&layer)).unwrap();
        let dt = t0.elapsed();
        renderer.end_frame().unwrap();
        *slot = dt.as_secs_f64() * 1000.0;
        // Every timed run must be real work: 8,294,400 opaque pixels.
        assert_eq!(stats.pixels_opaque, u64::from(W) * u64::from(H), "run {i}");
        assert_eq!(stats.pixels_blended, 0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = samples[RUNS / 2];
    println!("4K single-surface composite: median {median:.3} ms over {RUNS} runs");
    assert!(
        median < 8.0,
        "EC violated: 4K single-surface composite took {median:.3} ms (median of {RUNS})"
    );

    // The fast path's correctness spot check: corners + center.
    let fb = renderer.readout();
    let want = |x: u32, y: u32| {
        let [r, g, b, _] = pattern_rgba(x, y);
        0xFF00_0000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    };
    assert_eq!(fb[0], want(0, 0));
    assert_eq!(fb[(W - 1) as usize], want(W - 1, 0));
    assert_eq!(fb[((H - 1) * W) as usize], want(0, H - 1));
    assert_eq!(fb[((H - 1) * W + W - 1) as usize], want(W - 1, H - 1));
    assert_eq!(fb[((H / 2) * W + W / 2) as usize], want(W / 2, H / 2));
}

#[test]
fn k4_damage_limited_composite_scales_down() {
    // Half-height damage composites half the pixels — the span machinery
    // must actually skip undamaged rows (budget check, generous 2x).
    let (data, geometry) = frame();
    let output = OutputDesc::new(W, H, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let view = BufferView::new(1, &data, geometry.clone()).unwrap();
    let layer = SurfaceLayer::new(
        view,
        Rect::new(0, 0, W, H),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::new(),
    );
    let damage = Region::from_rect(Rect::new(0, 0, W, H / 2));
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    renderer.submit(std::slice::from_ref(&layer)).unwrap();
    renderer.end_frame().unwrap();

    renderer.begin_frame(&output, &damage).unwrap();
    let t0 = Instant::now();
    let stats = renderer.submit(&[layer]).unwrap();
    let half_ms = t0.elapsed().as_secs_f64() * 1000.0;
    renderer.end_frame().unwrap();
    assert_eq!(stats.pixels_opaque, u64::from(W) * u64::from(H / 2));
    println!("4K half-damage composite: {half_ms:.3} ms");
    assert!(
        half_ms < 8.0,
        "half-damage 4K composite took {half_ms:.3} ms"
    );
}
