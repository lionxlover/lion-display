//! The plane-assignment solver's decision suite — every doctrine
//! rule proven against the reference mock device's real plane
//! inventory (the six-plane laptop preset, its IN_FORMATS capability
//! blobs, and its zpos table read through the property walk the
//! compositor performs at bring-up).

#![allow(clippy::too_many_lines)]

use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::color::{ColorDescription, Primaries};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_display::ids::{FbId, PlaneId};
use ldp_display::MockDevice;
use ldp_planes::{Assigner, Demotion, LayerFacts, OutputFacts, PlaneCaps, PlaneInventory};

const W: u32 = 1920;
const H: u32 = 1080;

fn output() -> OutputFacts {
    OutputFacts {
        width: W,
        height: H,
        format: FourCC::XRGB8888,
        color: ColorDescription::srgb_sdr(),
    }
}

fn fb(raw: u32) -> Option<FbId> {
    FbId::new(raw)
}

/// A fullscreen opaque XRGB8888 layer — the direct-scanout shape.
fn fullscreen(format: FourCC, f: u32) -> LayerFacts {
    LayerFacts {
        dest: Rect::new(0, 0, W, H),
        width: W,
        height: H,
        format,
        modifier: Modifier::LINEAR,
        transform: Transform::Normal,
        color: ColorDescription::srgb_sdr(),
        opacity: 1.0,
        opaque: Region::from_rect(Rect::new(0, 0, W, H)),
        fb: fb(f),
        needs_backdrop: false,
        styled: false,
        system: false,
    }
}

/// A dock-shaped strip at the bottom (1:1, opaque, not covering).
fn dock(f: u32) -> LayerFacts {
    let (dock_x, dock_y, dock_w, dock_h) = (560u32, 1040u32, 800u32, 32u32);
    LayerFacts {
        dest: Rect::new(dock_x as i32, dock_y as i32, dock_w, dock_h),
        width: dock_w,
        height: dock_h,
        format: FourCC::XRGB8888,
        modifier: Modifier::LINEAR,
        transform: Transform::Normal,
        color: ColorDescription::srgb_sdr(),
        opacity: 1.0,
        opaque: Region::from_rect(Rect::new(dock_x as i32, dock_y as i32, dock_w, dock_h)),
        fb: fb(f),
        needs_backdrop: false,
        styled: false,
        system: true,
    }
}

fn inventory() -> PlaneInventory {
    let dev = MockDevice::laptop_dual();
    PlaneInventory::collect(&dev, 0).expect("crtc 0 inventory")
}

fn solve(stack: &[LayerFacts]) -> ldp_planes::ScanoutPlan {
    Assigner.solve(stack, &inventory(), &output())
}

// ------------------------------------------------------------------
// The inventory walk
// ------------------------------------------------------------------

#[test]
fn inventory_reads_the_preset_topology() {
    let inv = inventory();
    // CRTC 0 (id 42): primary 50 at zpos 0, overlays 51/52 at 1/2,
    // cursor 55 — all feeding CRTC index 0.
    assert_eq!(
        inv.primary.as_ref().map(|p| p.info.id),
        Some(PlaneId::new(50).unwrap())
    );
    assert_eq!(inv.primary.as_ref().map(|p| p.zpos), Some(0));
    let overlay_ids: Vec<u32> = inv.overlays.iter().map(|p| p.info.id.raw()).collect();
    assert_eq!(overlay_ids, vec![51, 52]);
    assert_eq!(inv.overlays[0].zpos, 1);
    assert_eq!(inv.overlays[1].zpos, 2);
    assert_eq!(
        inv.cursor.as_ref().map(|p| p.info.id),
        Some(PlaneId::new(55).unwrap())
    );
    assert!(inv.below_primary.is_empty());
    // The capability walk is the IN_FORMATS truth: the preset's
    // CRTC-0 planes (primary 50 and overlays 51/52) share the
    // video-friendly cap set — XRGB (linear + Intel X tiling), ARGB,
    // and NV12 linear. CRTC 1's overlay 54 is XRGB-only (the split
    // the per-CRTC tests pin).
    assert!(inv
        .primary
        .as_ref()
        .unwrap()
        .supports(FourCC::XRGB8888, Modifier::LINEAR));
    assert!(inv
        .primary
        .as_ref()
        .unwrap()
        .supports(FourCC::NV12, Modifier::LINEAR));
    assert!(inv.overlays[0].supports(FourCC::NV12, Modifier::LINEAR));
    assert!(!inv.overlays[0].supports(FourCC::NV12, Modifier::INTEL_X));
    assert!(inv.overlays[0].supports(FourCC::XRGB8888, Modifier::INTEL_X));
}

#[test]
fn inventory_splits_per_crtc() {
    let dev = MockDevice::laptop_dual();
    let inv1 = PlaneInventory::collect(&dev, 1).expect("crtc 1 inventory");
    // CRTC 1 (id 43): primary 53, overlay 54 (XRGB-only — no NV12).
    assert_eq!(
        inv1.primary.as_ref().map(|p| p.info.id),
        Some(PlaneId::new(53).unwrap())
    );
    assert_eq!(inv1.overlays.len(), 1);
    assert_eq!(inv1.overlays[0].info.id, PlaneId::new(54).unwrap());
    assert!(!inv1.overlays[0].supports(FourCC::NV12, Modifier::LINEAR));
    assert!(inv1.cursor.is_none());
}

#[test]
fn inventory_respects_the_crtc_mask() {
    let dev = MockDevice::laptop_dual();
    // Plane 51 (CRTC 0's overlay) must not appear in CRTC 1's set.
    let inv1 = PlaneInventory::collect(&dev, 1).expect("crtc 1 inventory");
    assert!(inv1
        .overlays
        .iter()
        .all(|p| p.info.id != PlaneId::new(51).unwrap()));
}

// ------------------------------------------------------------------
// The zero-composite frame
// ------------------------------------------------------------------

#[test]
fn a_fullscreen_opaque_fb_layer_is_direct_scanout() {
    let plan = solve(&[fullscreen(FourCC::XRGB8888, 3000)]);
    assert!(plan.zero_composite);
    assert_eq!(plan.composite_count, 0);
    assert!(plan.demotions.is_empty());
    assert_eq!(plan.assignments.len(), 1);
    let a = &plan.assignments[0];
    assert_eq!(a.plane, PlaneId::new(50).unwrap());
    assert_eq!(a.role, ldp_planes::PlaneRole::Primary);
    assert_eq!(a.fb, FbId::new(3000).unwrap());
    assert_eq!(a.zpos, 0);
    assert_eq!(a.src, (0, 0, W, H));
    assert_eq!(a.dst, (0, 0, W, H));
}

#[test]
fn fullscreen_client_plus_plain_dock_is_still_zero_composite() {
    // The macOS MPO shape: the client's buffer on the primary, the
    // dock's own pixels on an overlay above it — zero GPU passes.
    let plan = solve(&[fullscreen(FourCC::XRGB8888, 3000), dock(3001)]);
    assert!(plan.zero_composite);
    assert_eq!(plan.assignments.len(), 2);
    assert_eq!(plan.assignments[0].plane, PlaneId::new(50).unwrap());
    assert_eq!(plan.assignments[1].plane, PlaneId::new(51).unwrap());
    assert_eq!(plan.assignments[1].zpos, 1);
    assert_eq!(plan.zpos_ladder(), vec![0, 1]);
    assert_eq!(plan.composite_count, 0);
    assert!(plan.report().contains("zero-composite"));
}

#[test]
fn fullscreen_nv12_video_is_direct_scanout_on_this_driver() {
    // The preset's primary plane takes NV12+LINEAR (the
    // video-friendly driver class): fullscreen 4:2:0 video scans out
    // untouched — zero GPU passes, zero decode, the video player's
    // own buffer on the panel. This is the shape macOS hands AVPlayer
    // and Windows hands MPO.
    let plan = solve(&[fullscreen(FourCC::NV12, 3000)]);
    assert!(plan.zero_composite);
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].plane, PlaneId::new(50).unwrap());
    assert_eq!(plan.assignments[0].role, ldp_planes::PlaneRole::Primary);
    assert_eq!(plan.composite_count, 0);
}

#[test]
fn nv12_the_primary_refuses_rides_the_lowest_overlay() {
    // The synthetic driver whose primary is XRGB-only but whose
    // overlay takes NV12: the fullscreen opaque video covers the
    // stale canvas entirely, so the whole frame rides overlays —
    // zero composite passes, the video player's buffer untouched on
    // the panel (the old-driver fullscreen-video shape).
    let inv = synthetic_inventory(&[&[
        (FourCC::XRGB8888, Modifier::LINEAR),
        (FourCC::NV12, Modifier::LINEAR),
    ]]);
    let plan = Assigner.solve(&[fullscreen(FourCC::NV12, 3000)], &inv, &output());
    assert!(plan.zero_composite);
    assert_eq!(plan.composite_count, 0);
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].role, ldp_planes::PlaneRole::Overlay);
    assert_eq!(plan.assignments[0].zpos, 1);
    assert_eq!(
        plan.report(),
        "planes: 1 assigned, 0 composite passes (zero-composite frame)"
    );
}

#[test]
fn a_solo_holed_layer_still_composites_over_the_canvas() {
    // The overlay-bottom shape demands full opaque coverage: holes
    // would show the stale canvas through, so a holed solo layer
    // composites with the reason named.
    let inv = synthetic_inventory(&[&[
        (FourCC::XRGB8888, Modifier::LINEAR),
        (FourCC::NV12, Modifier::LINEAR),
    ]]);
    let mut holed = fullscreen(FourCC::NV12, 3000);
    holed.opaque = Region::from_rect(Rect::new(0, 0, W, H))
        .subtract(&Region::from_rect(Rect::new(10, 10, 20, 20)));
    let plan = Assigner.solve(&[holed], &inv, &output());
    assert!(!plan.zero_composite);
    assert_eq!(plan.composite_count, 1);
    assert_eq!(
        plan.demotions.first().map(|(_, d)| *d),
        Some(Demotion::BottomNotCovering)
    );
}

#[test]
fn nv12_video_over_a_composited_desktop_rides_the_overlay() {
    // SDR wallpaper (no fb — CPU window) beneath fullscreen NV12
    // video: the wallpaper composites onto the canvas, the video
    // rides overlay 51 above it.
    let mut wallpaper = fullscreen(FourCC::XRGB8888, 3000);
    wallpaper.fb = None; // CPU window
    let plan = solve(&[wallpaper, fullscreen(FourCC::NV12, 3001)]);
    assert!(!plan.zero_composite);
    assert_eq!(plan.composite_count, 1);
    assert_eq!(plan.demotions, vec![(0, Demotion::NoFb)]);
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].plane, PlaneId::new(51).unwrap());
    assert_eq!(plan.assignments[0].zpos, 1);
}

// ------------------------------------------------------------------
// The split-point doctrine
// ------------------------------------------------------------------

#[test]
fn a_translucent_middle_layer_forces_the_layers_under_it_to_composite() {
    // [fullscreen fb, translucent HUD, fullscreen fb]: the HUD
    // composites, so the bottom composites beneath it — and the top
    // layer rides the overlay above the canvas.
    let mut hud = fullscreen(FourCC::ARGB8888, 3001);
    hud.opacity = 0.5;
    let plan = solve(&[
        fullscreen(FourCC::XRGB8888, 3000),
        hud,
        fullscreen(FourCC::XRGB8888, 3002),
    ]);
    assert!(!plan.zero_composite);
    assert_eq!(plan.composite_count, 2);
    assert_eq!(
        plan.demotions,
        vec![(0, Demotion::BelowSplit), (1, Demotion::Translucent)]
    );
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].layer, 2);
    assert_eq!(plan.assignments[0].plane, PlaneId::new(51).unwrap());
}

#[test]
fn a_frosted_dock_needs_the_backdrop_and_demotes_the_whole_stack() {
    // The styled dock's material samples the composed backdrop: it
    // composites by definition, and the client beneath it must render
    // into the canvas under it.
    let mut frost = dock(3001);
    frost.needs_backdrop = true;
    let plan = solve(&[fullscreen(FourCC::XRGB8888, 3000), frost]);
    assert!(!plan.zero_composite);
    assert_eq!(plan.composite_count, 2);
    assert_eq!(
        plan.demotions,
        vec![(0, Demotion::BelowSplit), (1, Demotion::NeedsBackdrop)]
    );
}

#[test]
fn the_overlay_budget_pushes_the_split_up() {
    // Five capable layers (bottom covering + four strips) with two
    // overlay slots: zero-composite needs four overlays — impossible —
    // so the split lands where the suffix fits (the top two ride).
    let stack = vec![
        fullscreen(FourCC::XRGB8888, 3000),
        dock(3001),
        dock(3002),
        dock(3003),
        dock(3004),
    ];
    let plan = solve(&stack);
    assert!(!plan.zero_composite);
    assert_eq!(plan.assignments.len(), 2);
    assert_eq!(plan.assignments[0].layer, 3);
    assert_eq!(plan.assignments[1].layer, 4);
    assert_eq!(plan.assignments[0].plane, PlaneId::new(51).unwrap());
    assert_eq!(plan.assignments[1].plane, PlaneId::new(52).unwrap());
    assert_eq!(plan.composite_count, 3);
    assert_eq!(plan.demotions[0], (0, Demotion::BelowSplit));
}

// ------------------------------------------------------------------
// The eligibility table (one demotion per rule)
// ------------------------------------------------------------------

#[test]
fn every_eligibility_rule_reports_its_demotion() {
    // No fb.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.fb = None;
    assert_eq!(solve(&[layer]).demotions, vec![(0, Demotion::NoFb)]);

    // Scaled placement.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.width = 960;
    layer.height = 540;
    assert_eq!(solve(&[layer]).demotions, vec![(0, Demotion::Scaled)]);

    // Transformed.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.transform = Transform::Rot90;
    assert_eq!(solve(&[layer]).demotions, vec![(0, Demotion::Transformed)]);

    // Color mismatch.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    let mut p3 = ColorDescription::srgb_sdr();
    p3.primaries = Primaries::DciP3;
    layer.color = p3;
    assert_eq!(
        solve(&[layer]).demotions,
        vec![(0, Demotion::ColorMismatch)]
    );

    // Out of bounds (partially offscreen).
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.dest = Rect::new(-100, 0, W, H);
    assert_eq!(solve(&[layer]).demotions, vec![(0, Demotion::OutOfBounds)]);

    // Translucent surface opacity.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.opacity = 0.99;
    assert_eq!(solve(&[layer]).demotions, vec![(0, Demotion::Translucent)]);

    // Unsupported (format, modifier) pair on every plane.
    let mut layer = fullscreen(FourCC::NV12, 3000);
    layer.modifier = Modifier::INTEL_X;
    assert_eq!(
        solve(&[layer]).demotions,
        vec![(0, Demotion::FormatUnsupported)]
    );

    // Opaque holes in the bottom layer.
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    let holed = Region::from_rect(Rect::new(0, 0, W, H))
        .subtract(&Region::from_rect(Rect::new(100, 100, 200, 200)));
    layer.opaque = holed;
    // Still plane-capable as an overlay — but as the solo bottom
    // layer it cannot cover, so it composites with the reason named.
    assert_eq!(
        solve(&[layer]).demotions,
        vec![(0, Demotion::BottomNotCovering)]
    );
}

#[test]
fn the_color_rule_cuts_both_ways_for_hdr() {
    // An sRGB layer cannot ride a PQ canvas (no per-plane color
    // management in v1)…
    let pq_output = OutputFacts {
        width: W,
        height: H,
        format: FourCC::XRGB8888,
        color: ColorDescription::pq_hdr(),
    };
    let srgb_layer = fullscreen(FourCC::XRGB8888, 3000);
    let inv = inventory();
    let plan = Assigner.solve(&[srgb_layer], &inv, &pq_output);
    assert_eq!(
        plan.demotions.first().map(|(_, d)| *d),
        Some(Demotion::ColorMismatch)
    );

    // …and a PQ fullscreen layer on the PQ canvas scans out directly —
    // the HDR video case, hardware-composited on the PQ plane.
    let mut pq_layer = fullscreen(FourCC::XRGB8888, 3000);
    pq_layer.color = pq_output.color;
    let plan = Assigner.solve(&[pq_layer], &inv, &pq_output);
    assert!(plan.zero_composite);
}

// ------------------------------------------------------------------
// The search: zpos monotonicity and backtracking
// ------------------------------------------------------------------

/// Build a synthetic inventory: overlays with chosen capabilities.
fn synthetic_inventory(overlay_caps: &[&[(FourCC, Modifier)]]) -> PlaneInventory {
    use ldp_display::plane::{FormatModifier, InFormats};
    let mk =
        |raw: u32, kind: ldp_display::plane::PlaneType, caps: &[(FourCC, Modifier)], zpos: u64| {
            let pairs: Vec<FormatModifier> = caps
                .iter()
                .map(|&(f, m)| FormatModifier {
                    format: f,
                    modifier: m,
                })
                .collect();
            let formats = {
                let mut v: Vec<FourCC> = caps.iter().map(|&(f, _)| f).collect();
                v.sort_by_key(|f| f.code());
                v.dedup();
                v
            };
            PlaneCaps {
                info: ldp_display::plane::PlaneInfo {
                    id: PlaneId::new(raw).expect("nonzero"),
                    kind,
                    possible_crtcs: ldp_display::ids::CrtcMask::covering(1),
                    current_crtc: None,
                    formats,
                    in_formats: Some(InFormats::parse(&InFormats::encode(&pairs)).unwrap()),
                },
                zpos,
            }
        };
    let mut overlays = Vec::new();
    for (i, caps) in overlay_caps.iter().enumerate() {
        overlays.push(mk(
            100 + i as u32 * 7,
            ldp_display::plane::PlaneType::Overlay,
            caps,
            (i + 1) as u64,
        ));
    }
    PlaneInventory {
        crtc_index: 0,
        primary: Some(mk(
            50,
            ldp_display::plane::PlaneType::Primary,
            &[(FourCC::XRGB8888, Modifier::LINEAR)],
            0,
        )),
        overlays,
        cursor: None,
        below_primary: Vec::new(),
    }
}

#[test]
fn zpos_monotonicity_prevents_a_capability_inversion() {
    // Overlay A (zpos 1) takes only XRGB; overlay B (zpos 2) takes
    // XRGB and NV12. Stack: [covering XRGB, NV12, XRGB]. The NV12
    // layer can ONLY use B (zpos 2), but the XRGB above it would then
    // need a zpos above 2 — impossible. The solver must not produce
    // the inverted stacking; the correct plan offloads only the top
    // XRGB on A and composites the rest.
    let inv = synthetic_inventory(&[
        &[(FourCC::XRGB8888, Modifier::LINEAR)],
        &[
            (FourCC::XRGB8888, Modifier::LINEAR),
            (FourCC::NV12, Modifier::LINEAR),
        ],
    ]);
    let stack = vec![
        fullscreen(FourCC::XRGB8888, 3000),
        fullscreen(FourCC::NV12, 3001),
        fullscreen(FourCC::XRGB8888, 3002),
    ];
    let plan = Assigner.solve(&stack, &inv, &output());
    assert!(!plan.zero_composite);
    // Only the top XRGB rides — on overlay A (zpos 1).
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].layer, 2);
    assert_eq!(plan.assignments[0].zpos, 1);
    assert_eq!(plan.composite_count, 2);
}

#[test]
fn backtracking_finds_the_swap_when_both_planes_take_everything() {
    // Two identical-capability overlays: the DFS's first choice for
    // the NV12 layer is A (zpos 1), leaving B (zpos 2) for the XRGB
    // above — the correct monotone stacking without backtracking. But
    // when the candidates are ordered so the naive pick would strand
    // the second layer (A exhausted), the search swaps on its own.
    // Here: overlay A takes NV12 only; overlay B takes NV12 and XRGB.
    // Stack: [covering XRGB, NV12, XRGB]. NV12 can use A(1) or B(2);
    // the XRGB above can only use B(2) — so NV12 must take A.
    let inv = synthetic_inventory(&[
        &[(FourCC::NV12, Modifier::LINEAR)],
        &[
            (FourCC::NV12, Modifier::LINEAR),
            (FourCC::XRGB8888, Modifier::LINEAR),
        ],
    ]);
    let stack = vec![
        fullscreen(FourCC::XRGB8888, 3000),
        fullscreen(FourCC::NV12, 3001),
        fullscreen(FourCC::XRGB8888, 3002),
    ];
    let plan = Assigner.solve(&stack, &inv, &output());
    assert!(plan.zero_composite);
    assert_eq!(plan.assignments.len(), 3);
    // NV12 (layer 1) on A at zpos 1; XRGB (layer 2) on B at zpos 2.
    assert_eq!(plan.assignments[1].zpos, 1);
    assert_eq!(plan.assignments[2].zpos, 2);
    assert_eq!(plan.zpos_ladder(), vec![0, 1, 2]);
}

#[test]
fn solving_is_deterministic() {
    let stack = vec![
        fullscreen(FourCC::XRGB8888, 3000),
        dock(3001),
        fullscreen(FourCC::NV12, 3002),
    ];
    let a = solve(&stack);
    let b = solve(&stack);
    assert_eq!(a, b);
}

#[test]
fn an_empty_stack_is_the_default_plan() {
    let plan = solve(&[]);
    assert!(!plan.zero_composite);
    assert_eq!(plan.composite_count, 0);
    assert!(plan.assignments.is_empty());
}

#[test]
fn the_cursor_plane_is_reserved() {
    // Plane 55 (ARGB cursor) never carries content layers even for a
    // format it supports.
    let plan = solve(&[
        fullscreen(FourCC::ARGB8888, 3000),
        dock(3001),
        dock(3002),
        dock(3003),
    ]);
    for a in &plan.assignments {
        assert_ne!(a.plane, PlaneId::new(55).unwrap());
    }
}

#[test]
fn a_styled_layer_composites_because_its_pixels_do_not_exist() {
    // The Liquid styling (rounded corners, soft shadow) folds
    // coverage and casts imagery *in the render path* — a plane would
    // scan the buffer out verbatim, a rounded window going square.
    // The demotion names it.
    let mut styled = fullscreen(FourCC::XRGB8888, 3000);
    styled.styled = true;
    let plan = solve(&[styled]);
    assert!(!plan.zero_composite);
    assert_eq!(plan.assignments.len(), 0);
    assert_eq!(plan.demotions, vec![(0, Demotion::Styled)]);
}

#[test]
fn the_report_names_the_first_demotion() {
    let mut layer = fullscreen(FourCC::XRGB8888, 3000);
    layer.fb = None;
    let plan = solve(&[layer]);
    assert!(plan.report().contains("cpu-window buffer"));
    let plan = solve(&[fullscreen(FourCC::XRGB8888, 3000)]);
    assert!(plan.report().contains("zero-composite"));
}
