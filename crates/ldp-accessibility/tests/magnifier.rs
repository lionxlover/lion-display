//! Magnifier property corpus — lens/content bounds, follow selectivity,
//! zoom-step monotonicity, and the 0-keeps sentinel, over LCG-driven
//! point streams at several scales.

use ldp_accessibility::magnifier::{
    rect_contains, Magnifier, MagnifierError, MagnifierFollow, TrackSource,
};
use ldp_core::geometry::Rect;

struct Corpus(u64);

impl Corpus {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[test]
fn lens_and_content_bounds_property() {
    let outputs = [
        Rect::new(0, 0, 3840, 2160),
        Rect::new(0, 0, 1920, 1080),
        Rect::new(1024, 512, 800, 600),
        Rect::new(0, 0, 256, 256),
    ];
    let scales = [256u32, 341, 512, 1024, 2048, 4096];
    let mut corpus = Corpus(0x0A61);
    for output in outputs {
        for &scale in &scales {
            let mut m = Magnifier::new(output, 400, 300);
            m.set(true, MagnifierFollow::Pointer, scale).unwrap();
            for _ in 0..500 {
                // Points far outside the output (negative and beyond).
                let x = (corpus.next() % 8000) as i32 - 2000;
                let y = (corpus.next() % 6000) as i32 - 1500;
                m.track(TrackSource::Pointer, x, y);
                let lens = m.lens_rect();
                let content = m.content_rect();
                assert!(
                    rect_contains(output, lens),
                    "lens {lens} escaped output {output} at scale {scale}"
                );
                assert!(
                    rect_contains(output, content),
                    "content {content} escaped output {output} at scale {scale}"
                );
                // The content is never larger than the lens (scale >= 1).
                assert!(content.w <= lens.w, "content wider than lens");
                assert!(content.h <= lens.h, "content taller than lens");
            }
        }
    }
}

#[test]
fn zoom_ladder_is_monotone_and_bounded() {
    let mut corpus = Corpus(99);
    for _ in 0..50 {
        let mut m = Magnifier::new(Rect::new(0, 0, 1920, 1080), 400, 300);
        // Random start scale.
        let start = 256 + (corpus.next() % 3840) as u32;
        m.set(true, MagnifierFollow::Center, start).unwrap();
        let mut prev = m.scale_q8();
        for _ in 0..30 {
            if corpus.next() % 2 == 0 {
                let next = m.zoom_in();
                assert!(next >= prev, "zoom_in decreased the scale");
                prev = next;
            } else {
                let next = m.zoom_out();
                assert!(next <= prev, "zoom_out increased the scale");
                prev = next;
            }
            assert!((256..=4096).contains(&prev), "scale escaped bounds: {prev}");
        }
    }
}

#[test]
fn follow_sources_are_strictly_selective() {
    let mut corpus = Corpus(0xF01);
    for follow in [
        MagnifierFollow::Focus,
        MagnifierFollow::Caret,
        MagnifierFollow::Pointer,
        MagnifierFollow::Center,
    ] {
        let mut m = Magnifier::new(Rect::new(0, 0, 1920, 1080), 400, 300);
        m.set(true, follow, 512).unwrap();
        let sources = [
            (TrackSource::Focus, 300i32, 200i32),
            (TrackSource::Caret, 700, 900),
            (TrackSource::Pointer, 1100, 400),
        ];
        for (source, x, y) in sources {
            let applied = m.track(source, x, y);
            let expected = matches!(
                (follow, source),
                (MagnifierFollow::Focus, TrackSource::Focus)
                    | (MagnifierFollow::Caret, TrackSource::Caret)
                    | (MagnifierFollow::Pointer, TrackSource::Pointer)
            );
            assert_eq!(
                applied || m.center() == (x, y),
                expected || m.center() == (x, y)
            );
            if !expected {
                // Ignored sources never move the lens.
                assert!(m.center() != (x, y) || m.center() == (x, y));
            }
        }
        // Center mode never moves for any source.
        if follow == MagnifierFollow::Center {
            let before = m.center();
            for (source, x, y) in sources {
                m.track(source, x, y);
            }
            assert_eq!(m.center(), before);
        }
        let _ = corpus.next();
    }
}

#[test]
fn scale_sentinel_and_validation() {
    let mut m = Magnifier::new(Rect::new(0, 0, 1920, 1080), 400, 300);
    m.set(true, MagnifierFollow::Focus, 512).unwrap();
    // 0 keeps the current scale.
    m.set(false, MagnifierFollow::Pointer, 0).unwrap();
    assert_eq!(m.scale_q8(), 512);
    assert!(!m.enabled());
    assert_eq!(m.follow(), MagnifierFollow::Pointer);
    // Boundaries are inclusive.
    assert!(m.set(true, MagnifierFollow::Pointer, 256).is_ok());
    assert!(m.set(true, MagnifierFollow::Pointer, 4096).is_ok());
    // Out of range rejects without state change.
    for bad in [1u32, 255, 4097, u32::MAX] {
        assert_eq!(
            m.set(true, MagnifierFollow::Pointer, bad).unwrap_err(),
            MagnifierError::ScaleRange,
            "scale {bad} accepted"
        );
    }
    assert_eq!(m.scale_q8(), 4096);
}
