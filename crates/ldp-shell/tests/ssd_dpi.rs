//! SSD insets correct on mixed-DPI (Phase 12 EC).
//!
//! A mixed-DPI session: outputs at different scale factors, windows
//! migrating between them. The contract under test:
//!
//! * insets derive *per output* from one logical metric table;
//! * physical insets ceil-cover the logical reservation (never
//!   under-cover, error < 1 physical px);
//! * frame = content + insets exactly, in both spaces, at every
//!   scale — no fractional residue;
//! * a toplevel migrating outputs gets a fresh proposal with the new
//!   output's insets (and a new serial — the client must re-ack);
//! * the un-scale round trip loses at most one physical pixel per
//!   edge (the ceil is honest).

#![forbid(unsafe_code)]

mod common;

use common::{policy_inputs_at_scale, scale, Rng};
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_core::scale::ScaleFactor;
use ldp_shell::event::ShellEvent;
use ldp_shell::toplevel::{PolicyInputs, Toplevel};
use ldp_shell::{DecorationMode, Insets, SsdMetrics, WindowKey};

const SCALES: &[f32] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

#[test]
fn insets_ceil_cover_at_every_scale() {
    for &v in SCALES {
        let s = scale(v);
        for mode in [DecorationMode::Server, DecorationMode::Client] {
            let i = SsdMetrics::LION.insets(mode, s);
            let (lw, lt) = match mode {
                DecorationMode::Server => (
                    f64::from(SsdMetrics::LION.border) * f64::from(v),
                    f64::from(SsdMetrics::LION.border + SsdMetrics::LION.title_bar) * f64::from(v),
                ),
                DecorationMode::Client => {
                    let hz = f64::from(SsdMetrics::LION.hit_zone) * f64::from(v);
                    (hz, hz)
                }
            };
            // Never under-cover, error strictly below one pixel.
            assert!(
                f64::from(i.left) >= lw,
                "left under-covers at {v}x {mode:?}: {} < {lw}",
                i.left
            );
            assert!(
                f64::from(i.top) >= lt,
                "top under-covers at {v}x {mode:?}: {} < {lt}",
                i.top
            );
            assert!((f64::from(i.top) - lt) < 1.0, "top error >= 1px at {v}x");
            assert!((f64::from(i.left) - lw) < 1.0, "left error >= 1px at {v}x");
            // Symmetric sides.
            assert_eq!(i.left, i.right);
        }
    }
}

#[test]
fn frame_geometry_is_integral_at_every_scale() {
    for &v in SCALES {
        let s = scale(v);
        let i = SsdMetrics::LION.insets(DecorationMode::Server, s);
        // A content rect in device pixels (already integral).
        let content = Rect::new(64, 64, s.scale_px_up(800), s.scale_px_up(600));
        let frame = i.frame_around(content);
        // Frame = content + insets exactly.
        assert_eq!(frame.x, content.x - i.left as i32);
        assert_eq!(frame.y, content.y - i.top as i32);
        assert_eq!(frame.w, content.w + i.width());
        assert_eq!(frame.h, content.h + i.height());
        // And back.
        assert_eq!(i.content_inside(frame), content);
        // The un-scale round trip: device over-cover is bounded by the
        // ceil semantics (content < 1 px + two inset edges < 1 px
        // each), and unscale's floor loses < 1 logical px — so the
        // logical width stays within [800, 800 + 3/scale + 1].
        let logical_w = s.unscale_px(frame.w);
        assert!(logical_w >= 800, "lost more than insets allow at {v}x");
        let bound = 800 + (3.0f32 / v).ceil() as u32 + 1;
        assert!(
            logical_w <= bound,
            "gained pixels at {v}x: {logical_w} > {bound}"
        );
    }
}

#[test]
fn maximized_geometry_respects_scale() {
    for &v in SCALES {
        let s = scale(v);
        let inputs = policy_inputs_at_scale(s, ObjectId::from_wire(0x30));
        let mut t = Toplevel::new(WindowKey::new(1));
        t.maximize();
        let c = t.propose(&inputs);
        let insets = SsdMetrics::LION.insets(DecorationMode::Server, s);
        // Content + insets reconstructs the workspace area exactly.
        assert_eq!(
            c.width + insets.width(),
            inputs.workspace_area.w,
            "width+insets != area at {v}x"
        );
        assert_eq!(
            c.height + insets.height(),
            inputs.workspace_area.h,
            "height+insets != area at {v}x"
        );
    }
}

#[test]
fn migrating_outputs_rederives_insets_with_new_serials() {
    // The mixed-DPI session: output A at 1x, output B at 2x.
    let out_a = ObjectId::from_wire(0x30);
    let out_b = ObjectId::from_wire(0x31);
    let mut t = Toplevel::new(WindowKey::new(1));
    t.maximize();

    // Start on output A.
    let inputs_a = policy_inputs_at_scale(scale(1.0), out_a);
    let ca = t.propose(&inputs_a);
    assert_eq!(ca.insets.top, 29);
    assert_eq!(ca.output, Some(out_a));
    t.ack_configure(ca.serial).unwrap();
    assert_eq!(t.commit(), Some(ca));

    // Migrate to output B (2x): the proposal carries B's insets.
    let inputs_b = policy_inputs_at_scale(scale(2.0), out_b);
    let cb = t.propose(&inputs_b);
    assert_eq!(cb.insets.top, 58, "insets must re-derive at 2x");
    assert_eq!(cb.insets.left, 2);
    assert_eq!(cb.output, Some(out_b));
    // And a fresh serial the client must ack.
    assert!(cb.serial.after(ca.serial));
    assert_eq!(t.commit(), None, "nothing realized without a new ack");
    t.ack_configure(cb.serial).unwrap();
    assert_eq!(t.commit(), Some(cb));

    // The wire event reflects the migration honestly.
    let e = ShellEvent::toplevel_configure(&cb);
    match &e {
        ShellEvent::ToplevelConfigure {
            inset_top,
            inset_left,
            output,
            ..
        } => {
            assert_eq!(*inset_top, 58);
            assert_eq!(*inset_left, 2);
            assert_eq!(*output, Some(out_b));
        }
        _ => panic!("wrong variant"),
    }
    // The 1x insets are the odd ones out: different from 2x (the
    // mixed-DPI property in one line).
    assert_ne!(ca.insets, cb.insets);
}

#[test]
fn csd_and_ssd_coexist_across_scales() {
    // Two windows on the same 2x output, one SSD one CSD: different
    // insets, both ceil-correct.
    let s = scale(2.0);
    let ssd = SsdMetrics::LION.insets(DecorationMode::Server, s);
    let csd = SsdMetrics::LION.insets(DecorationMode::Client, s);
    assert_eq!((ssd.top, ssd.left), (58, 2));
    assert_eq!((csd.top, csd.left), (10, 10));
    // The frame margin (shadow/hit zone) is shared system chrome.
    let fm = SsdMetrics::LION.frame_margin(s);
    assert_eq!(fm.top, 16);
}

#[test]
fn fuzzed_scale_table_stays_honest() {
    // Random Q8.8 scales in a sane range: the ceil contract must hold
    // for every one of them, not just the pretty ones.
    let mut rng = Rng::seeded(0xD111);
    for _ in 0..2_000 {
        let q8 = u32::try_from(256 + rng.below(768)).unwrap(); // 1.0 .. 4.0
        let Some(s) = ScaleFactor::from_q8(q8) else {
            continue;
        };
        let v = f64::from(s.to_q8()) / 256.0;
        let i = SsdMetrics::LION.insets(DecorationMode::Server, s);
        let lt = f64::from(SsdMetrics::LION.border + SsdMetrics::LION.title_bar) * v;
        let lw = f64::from(SsdMetrics::LION.border) * v;
        assert!(f64::from(i.top) >= lt && f64::from(i.top) < lt + 1.0);
        assert!(f64::from(i.left) >= lw && f64::from(i.left) < lw + 1.0);
        // Insets are physical: integral by type. The frame relation
        // holds for arbitrary content.
        let content = Rect::new(3, 5, u32::try_from(rng.below(2000)).unwrap() + 1, 777);
        let frame = i.frame_around(content);
        assert_eq!(frame.w, content.w + i.width());
        assert_eq!(frame.h, content.h + i.height());
        let _ = Insets::uniform(i.top);
    }
}

#[test]
fn fullscreen_zero_insets_at_every_scale() {
    for &v in SCALES {
        let s = scale(v);
        let inputs = PolicyInputs {
            workspace_area: Rect::new(0, 0, 1920, 1040),
            output_size: (1920, 1080),
            metrics: SsdMetrics::LION,
            decoration: DecorationMode::Server,
            scale: s,
            workspace: 0,
            output: Some(ObjectId::from_wire(0x30)),
        };
        let mut t = Toplevel::new(WindowKey::new(1));
        t.fullscreen();
        let c = t.propose(&inputs);
        assert_eq!(c.insets, Insets::ZERO, "fullscreen insets at {v}x");
        assert_eq!((c.width, c.height), (1920, 1080));
    }
}
