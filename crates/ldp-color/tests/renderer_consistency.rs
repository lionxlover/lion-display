//! Cross-consistency with the renderer's Phase 8 hook.
//!
//! The renderer carries its own tolerance-anchored copies of the
//! transfer curves and the primaries-matrix derivation (its per-pixel
//! pipeline hook, delivered in Phase 8 — before `ldp-color` existed).
//! This suite pins the two implementations **equal**, so they can
//! never drift apart silently: the canonical crate and the renderer
//! hook must produce the same decode/encode values and the same
//! conversion matrices, forever.

use ldp_color::matrix::primaries_matrix;
use ldp_color::transfer::{decode_transfer, encode_transfer};
use ldp_core::color::{Primaries, TransferFunction};
use ldp_renderer::primaries_matrix as renderer_primaries_matrix;
use ldp_renderer::{decode_transfer as renderer_decode, encode_transfer as renderer_encode};

#[test]
fn transfer_curves_are_identical() {
    // Same formulas, same constants, same branch order → bit-identical
    // results; the tolerance only absorbs hypothetical libm variance
    // between compilation units (none observed).
    for tf in [
        TransferFunction::Linear,
        TransferFunction::Srgb,
        TransferFunction::Pq,
        TransferFunction::Hlg,
        TransferFunction::Gamma22,
        TransferFunction::Gamma28,
    ] {
        for i in 0..=256 {
            let v = i as f32 / 256.0;
            let ours = decode_transfer(tf, v);
            let theirs = renderer_decode(tf, v);
            assert!(
                (ours - theirs).abs() <= 1e-7,
                "{tf:?} decode drift at {v}: {ours} vs {theirs}"
            );
            let ours_e = encode_transfer(tf, v);
            let theirs_e = renderer_encode(tf, v);
            assert!(
                (ours_e - theirs_e).abs() <= 1e-7,
                "{tf:?} encode drift at {v}: {ours_e} vs {theirs_e}"
            );
        }
        // Out-of-domain clamps agree too.
        for v in [-0.5f32, 1.5] {
            assert_eq!(decode_transfer(tf, v), renderer_decode(tf, v));
            assert_eq!(encode_transfer(tf, v), renderer_encode(tf, v));
        }
    }
}

#[test]
fn primaries_matrices_are_identical() {
    for src in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
        for dst in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let ours = primaries_matrix(src, dst);
            let theirs = renderer_primaries_matrix(src, dst);
            for i in 0..9 {
                assert!(
                    (ours[i] - theirs[i]).abs() <= 1e-6,
                    "{src:?}→{dst:?} drift [{i}]: {} vs {}",
                    ours[i],
                    theirs[i]
                );
            }
            // Identity fast paths are exactly equal (both are the
            // exact identity constant).
            if src == dst {
                assert_eq!(ours, theirs);
            }
        }
    }
}
