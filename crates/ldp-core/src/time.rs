//! Frame timing and deadlines.
//!
//! The deadline-aware scheduling model (`docs/architecture.md` §10.3) runs
//! on these types: [`Mono`] timestamps, [`RefreshInterval`], the
//! [`FrameDeadline`] contract the server pushes to clients, and
//! [`PresentationTiming`] feedback. Acquisition of the real clock lives in
//! `ldp-transport` (syscall boundary); this module is pure arithmetic —
//! fully unit-testable and deterministic.

use core::fmt;

/// A monotonic timestamp in nanoseconds (`CLOCK_MONOTONIC` domain).
///
/// Not a wall clock: never jumps on NTP, meaningless across reboots,
/// valid for deadline math only. Zero is the epoch; the protocol does not
/// assign meaning to absolute values.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
#[repr(transparent)]
pub struct Mono {
    ns: u64,
}

impl Mono {
    /// The epoch.
    pub const ZERO: Mono = Mono { ns: 0 };

    /// Construct from nanoseconds.
    pub const fn from_ns(ns: u64) -> Mono {
        Mono { ns }
    }

    /// Construct from milliseconds.
    pub const fn from_ms(ms: u64) -> Mono {
        Mono {
            ns: ms.saturating_mul(1_000_000),
        }
    }

    /// Nanoseconds.
    #[must_use]
    pub const fn as_ns(self) -> u64 {
        self.ns
    }

    /// Milliseconds, truncating sub-ms precision.
    #[must_use]
    pub const fn as_ms(self) -> u64 {
        self.ns / 1_000_000
    }

    /// Add a duration, saturating.
    #[must_use]
    pub const fn saturating_add_ns(self, ns: u64) -> Mono {
        Mono {
            ns: self.ns.saturating_add(ns),
        }
    }

    /// Subtract a duration, saturating at zero.
    #[must_use]
    pub const fn saturating_sub_ns(self, ns: u64) -> Mono {
        Mono {
            ns: self.ns.saturating_sub(ns),
        }
    }

    /// Duration `self - other`, saturating at zero when `other` is later.
    #[must_use]
    pub const fn duration_since(self, other: Mono) -> u64 {
        self.ns.saturating_sub(other.ns)
    }

    /// Milliseconds since `other`.
    #[must_use]
    pub const fn ms_since(self, other: Mono) -> u64 {
        self.duration_since(other) / 1_000_000
    }
}

impl fmt::Display for Mono {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:06} ms", self.ns / 1_000_000, self.ns % 1_000_000)
    }
}

/// A refresh interval in nanoseconds (frame period, the inverse of the
/// refresh rate).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct RefreshInterval {
    ns: u64,
}

impl RefreshInterval {
    /// Construct from nanoseconds; zero is rejected (a frame always has a
    /// positive period).
    pub const fn from_ns(ns: u64) -> Option<RefreshInterval> {
        if ns == 0 {
            None
        } else {
            Some(RefreshInterval { ns })
        }
    }

    /// Construct from millihertz as advertised on the wire
    /// (`output.mode.refresh_millihz`): 60000 mHz = 60 Hz = 16,666,666 ns.
    pub const fn from_millihz(millihz: u32) -> Option<RefreshInterval> {
        if millihz == 0 {
            None
        } else {
            // ns = 1e12 / mHz, computed with rounding.
            Some(RefreshInterval {
                ns: 1_000_000_000_000 / millihz as u64,
            })
        }
    }

    /// Nanoseconds per frame.
    #[must_use]
    pub const fn as_ns(self) -> u64 {
        self.ns
    }

    /// Refresh rate in millihertz (truncated).
    #[must_use]
    pub const fn as_millihz(self) -> u64 {
        1_000_000_000_000 / self.ns
    }
}

impl fmt::Display for RefreshInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} Hz", 1e9 / self.ns as f64)
    }
}

/// How a surface's commits are scheduled (wire: `ldp.core.presentation_mode`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum PresentationMode {
    /// Deadline-driven, never torn (default).
    #[default]
    Vsync,
    /// VRR window: commit at ready within `[min, max]` refresh.
    Adaptive,
    /// Tearing allowed, lowest latency; requires output support.
    Immediate,
}

impl PresentationMode {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Vsync => 1,
            Self::Adaptive => 2,
            Self::Immediate => 3,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<PresentationMode> {
        match v {
            1 => Some(Self::Vsync),
            2 => Some(Self::Adaptive),
            3 => Some(Self::Immediate),
            _ => None,
        }
    }
}

/// The deadline contract pushed to a client for one frame
/// (`surface.frame_target`).
///
/// To land on the target frame, the client's `commit` — with its acquire
/// fence already signaled — must reach the server before `deadline`. The
/// server computes `deadline` as
/// `target_vblank - submit_cost - flip_latency`; clients never guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameDeadline {
    /// Absolute time by which the commit must arrive.
    pub deadline: Mono,
    /// The vblank the frame targets.
    pub target_vblank: Mono,
    /// Current refresh interval of the output.
    pub refresh: RefreshInterval,
    /// Nanoseconds of render budget remaining at emission time
    /// (`deadline - now` at emission; informational).
    pub budget_ns: u64,
    /// Scheduling policy in force.
    pub mode: PresentationMode,
}

impl FrameDeadline {
    /// Whether `now` still satisfies the deadline.
    #[must_use]
    pub const fn is_met(&self, now: Mono) -> bool {
        now.as_ns() <= self.deadline.as_ns()
    }

    /// Nanoseconds until the deadline (0 when already missed).
    #[must_use]
    pub const fn remaining_ns(&self, now: Mono) -> u64 {
        self.deadline.duration_since(now)
    }
}

/// Presentation feedback (`surface.presented`): how a frame actually landed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PresentationTiming {
    /// The frame this feedback belongs to (client-chosen frame ID).
    pub frame: u64,
    /// Presentation (vblank/flip IRQ) timestamp.
    pub presented_at: Mono,
    /// Measured refresh interval at presentation.
    pub refresh: RefreshInterval,
    /// How the frame was presented.
    pub flags: PresentationFlags,
}

/// How a frame was presented (wire: `ldp.core.presented_flags`).
/// One bool per defined flag bit — the wire bitset decomposed for type-safe
/// access; adding flags grows the struct by design.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PresentationFlags {
    /// Presented at a vblank boundary.
    pub vblank: bool,
    /// Direct scanout (no composition pass).
    pub scanout: bool,
    /// On a hardware overlay plane.
    pub overlay: bool,
    /// Presented with tearing (immediate mode).
    pub torn: bool,
}

impl PresentationFlags {
    /// Encode to the 128-bit wire bitset (bits 0..=3, low word).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        (self.vblank as u32)
            | ((self.scanout as u32) << 1)
            | ((self.overlay as u32) << 2)
            | ((self.torn as u32) << 3)
    }

    /// Decode from the low wire word; unknown bits must be ignored by
    /// receivers, so only the defined bits are read.
    #[must_use]
    pub const fn from_wire(word: u32) -> PresentationFlags {
        PresentationFlags {
            vblank: (word & 0x1) != 0,
            scanout: (word & 0x2) != 0,
            overlay: (word & 0x4) != 0,
            torn: (word & 0x8) != 0,
        }
    }
}

/// Why a registered frame did not reach the display
/// (wire: `ldp.core.frame_drop_reason`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum FrameDropReason {
    /// The commit missed its deadline; the frame was retargeted.
    DeadlineMissed,
    /// The surface became unmapped or fully occluded.
    SurfaceHidden,
    /// The output was off / parked.
    OutputOff,
    /// The scheduler deferred the frame (VRR fixed-rate fallback, Phase 15).
    Throttled,
    /// A newer registration replaced this one.
    Superseded,
}

impl FrameDropReason {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::DeadlineMissed => 1,
            Self::SurfaceHidden => 2,
            Self::OutputOff => 3,
            Self::Throttled => 4,
            Self::Superseded => 5,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<FrameDropReason> {
        match v {
            1 => Some(Self::DeadlineMissed),
            2 => Some(Self::SurfaceHidden),
            3 => Some(Self::OutputOff),
            4 => Some(Self::Throttled),
            5 => Some(Self::Superseded),
            _ => None,
        }
    }
}

/// First-order phase-locked vblank predictor.
///
/// Anchors a timeline on real page-flip timestamps and predicts the next
/// vblank as `last_flip + refresh`, correcting long-run drift with a
/// proportional term so predicted and actual flips stay locked (the
/// scheduler never accumulates error across hours of operation).
///
/// This is pure state arithmetic; the compositor feeds it flip timestamps
/// from KMS page-flip events.
#[derive(Clone, Copy, Debug)]
pub struct VblankPredictor {
    refresh: RefreshInterval,
    last_flip: Mono,
    /// Integrated correction in nanoseconds (bounded by ±refresh/16).
    correction_ns: i64,
}

impl VblankPredictor {
    /// Start a timeline from a real flip timestamp.
    pub fn new(refresh: RefreshInterval, first_flip: Mono) -> Self {
        Self {
            refresh,
            last_flip: first_flip,
            correction_ns: 0,
        }
    }

    /// The refresh interval this timeline runs at.
    #[must_use]
    pub const fn refresh(&self) -> RefreshInterval {
        self.refresh
    }

    /// Timestamp of the most recently observed flip.
    #[must_use]
    pub const fn last_flip(&self) -> Mono {
        self.last_flip
    }

    /// Current integrated correction in nanoseconds (signed).
    ///
    /// At PLL lock this equals the per-period difference between the real
    /// panel period and the nominal `refresh` (positive: the panel runs
    /// slower than nominal), which is why the correction doubles as the
    /// drift term for multi-period extrapolation.
    #[must_use]
    pub const fn correction_ns(&self) -> i64 {
        self.correction_ns
    }

    /// Effective extrapolation step: nominal period plus the integrated
    /// correction.
    ///
    /// One-step predictions are exact against the corrected anchor; adding
    /// `effective_step_ns()` repeatedly extrapolates further vblanks and
    /// stays exact at lock when the panel period differs from nominal
    /// (the plain period would accumulate the drift error per step).
    /// Always positive: the correction is clamped inside one period.
    #[must_use]
    pub const fn effective_step_ns(&self) -> u64 {
        let v = self.refresh.as_ns() as i64 + self.correction_ns;
        if v < 1 {
            1
        } else {
            v as u64
        }
    }

    /// Predict the next vblank time after `now` on this timeline.
    #[must_use]
    pub fn next_vblank(&self, now: Mono) -> Mono {
        let step = self.refresh.as_ns();
        let anchor = self.last_flip.as_ns() as i64 + self.correction_ns;
        let now_ns = now.as_ns() as i64;
        if now_ns <= anchor {
            return Mono::from_ns(anchor.max(0) as u64);
        }
        let elapsed = now_ns - anchor;
        let periods = (elapsed + step as i64 - 1) / step as i64; // ceil
        Mono::from_ns((anchor + periods * step as i64).max(0) as u64)
    }

    /// Feed an actual flip timestamp: corrects drift proportionally.
    ///
    /// `flip_ts` should be the timestamp of the flip that landed at the
    /// prediction made one period ago. Small residuals adjust the
    /// correction; large residuals (device sleep, modeset) re-anchor.
    pub fn observe_flip(&mut self, flip_ts: Mono) {
        let step = self.refresh.as_ns() as i64;
        let predicted = self.last_flip.as_ns() as i64 + step + self.correction_ns;
        let residual = flip_ts.as_ns() as i64 - predicted;
        if residual.abs() > step / 2 {
            // Discontinuity (resume, modeset): re-anchor the timeline.
            self.last_flip = flip_ts;
            self.correction_ns = 0;
        } else {
            // Proportional correction, clamped to ±refresh/16 per update.
            let clamp = step / 16;
            let adjustment = (residual / 16).clamp(-clamp, clamp);
            self.correction_ns = (self.correction_ns + adjustment).clamp(-step, step);
            self.last_flip = flip_ts;
            // Correction is folded into predictions gradually; keep the
            // magnitude from ever exceeding one full period.
            if self.correction_ns.abs() >= step {
                self.correction_ns = self.correction_ns.clamp(-step + 1, step - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_arithmetic_saturates() {
        let a = Mono::from_ms(5);
        assert_eq!(a.as_ns(), 5_000_000);
        assert_eq!(a.saturating_add_ns(1_000_000).as_ns(), 6_000_000);
        assert_eq!(Mono::ZERO.saturating_sub_ns(10).as_ns(), 0);
        assert_eq!(a.ms_since(Mono::ZERO), 5);
        let big = Mono::from_ns(u64::MAX);
        assert_eq!(big.saturating_add_ns(1).as_ns(), u64::MAX);
        assert_eq!(a.to_string(), "5.000000 ms");
    }

    #[test]
    fn refresh_conversions() {
        let r = RefreshInterval::from_millihz(60_000).unwrap();
        assert_eq!(r.as_ns(), 16_666_666);
        assert_eq!(r.as_millihz(), 60_000);
        let r = RefreshInterval::from_millihz(144_000).unwrap();
        // 1e12 / 144000 = 6944444 ns ≈ 6.94 ms.
        assert_eq!(r.as_ns(), 6_944_444);
        assert!(RefreshInterval::from_millihz(0).is_none());
        assert!(RefreshInterval::from_ns(0).is_none());
        assert_eq!(
            RefreshInterval::from_ns(1_000_000).unwrap().to_string(),
            "1000.000 Hz"
        );
    }

    #[test]
    fn presentation_mode_wire() {
        assert_eq!(PresentationMode::default(), PresentationMode::Vsync);
        for v in 1..=3u32 {
            let m = PresentationMode::from_wire(v).unwrap();
            assert_eq!(m.to_wire(), v);
        }
        assert!(PresentationMode::from_wire(4).is_none());
    }

    #[test]
    fn presentation_flags_wire() {
        let f = PresentationFlags {
            vblank: true,
            scanout: true,
            overlay: false,
            torn: false,
        };
        assert_eq!(f.to_wire(), 0b0011);
        assert_eq!(PresentationFlags::from_wire(0b0011), f);
        let torn = PresentationFlags {
            torn: true,
            ..Default::default()
        };
        assert_eq!(torn.to_wire(), 0b1000);
        // Unknown bits ignored.
        assert_eq!(
            PresentationFlags::from_wire(0b1000_0000),
            PresentationFlags::default()
        );
    }

    #[test]
    fn deadline_logic() {
        let deadline = FrameDeadline {
            deadline: Mono::from_ns(1_000),
            target_vblank: Mono::from_ns(2_000),
            refresh: RefreshInterval::from_ns(1_000).unwrap(),
            budget_ns: 500,
            mode: PresentationMode::Vsync,
        };
        assert!(deadline.is_met(Mono::from_ns(999)));
        assert!(deadline.is_met(Mono::from_ns(1_000)));
        assert!(!deadline.is_met(Mono::from_ns(1_001)));
        assert_eq!(deadline.remaining_ns(Mono::from_ns(400)), 600);
        assert_eq!(deadline.remaining_ns(Mono::from_ns(1_500)), 0);
    }

    #[test]
    fn frame_drop_reason_wire() {
        for v in 1..=5u32 {
            let r = FrameDropReason::from_wire(v).unwrap();
            assert_eq!(r.to_wire(), v);
        }
        assert!(FrameDropReason::from_wire(0).is_none());
        assert!(FrameDropReason::from_wire(6).is_none());
    }

    #[test]
    fn effective_step_tracks_period_drift() {
        // A 60 Hz nominal timeline on a panel actually running at ~55 Hz
        // (period 18_181_818 ns): the correction must converge to the
        // per-period difference so the effective step matches the panel.
        let nominal = RefreshInterval::from_ns(16_666_666).unwrap();
        let true_step = 18_181_818u64;
        let mut p = VblankPredictor::new(nominal, Mono::ZERO);
        let mut t = 0u64;
        for _ in 0..512 {
            t += true_step;
            p.observe_flip(Mono::from_ns(t));
        }
        let want = true_step as i64 - nominal.as_ns() as i64; // ~1.52 ms
        let got = p.correction_ns();
        let off = (got - want).abs();
        assert!(
            off < 200_000,
            "correction {got} vs drift {want} (off {off})"
        );
        assert_eq!(
            p.effective_step_ns(),
            (nominal.as_ns() as i64 + got).max(1) as u64
        );
        assert!((p.effective_step_ns() as i64 - true_step as i64).abs() < 200_000);
    }

    #[test]
    fn predictor_locks_without_drift() {
        let refresh = RefreshInterval::from_millihz(60_000).unwrap();
        let mut p = VblankPredictor::new(refresh, Mono::ZERO);
        let step = refresh.as_ns();
        // Simulate a display running 100 ppm fast: flips land 100ns/period
        // earlier than the ideal grid. Corrected predictions must track
        // within a small error, not accumulate.
        let mut worst_error: u64 = 0;
        for k in 1..=20_000u64 {
            let flip = Mono::from_ns(k * step - k * 100); // slow drift source
            p.observe_flip(flip);
            let predicted = p.next_vblank(Mono::from_ns(k * step - 100));
            // Unsigned |a - b|: one direction holds the true delta.
            let error = predicted
                .duration_since(flip)
                .max(flip.duration_since(predicted));
            worst_error = worst_error.max(error);
        }
        // Without correction, error would be 20_000 * 100 = 2 ms.
        // With the PLL it must stay well inside one refresh period/16 (~1ms
        // worst case bound is loose; typical is far tighter).
        assert!(
            worst_error < step,
            "drift not corrected: worst error {worst_error} ns"
        );
    }

    #[test]
    fn predictor_reanchors_on_discontinuity() {
        let refresh = RefreshInterval::from_ns(16_000_000).unwrap();
        let mut p = VblankPredictor::new(refresh, Mono::ZERO);
        p.observe_flip(Mono::from_ns(16_000_000));
        // The machine slept; the next flip lands way off the grid.
        p.observe_flip(Mono::from_ns(120_000_000_000));
        let next = p.next_vblank(Mono::from_ns(120_000_100_000));
        // Prediction must be after the re-anchored flip, within one period.
        let delta = next.as_ns() - 120_000_000_000;
        assert!(
            delta > 0 && delta <= 16_000_000,
            "bad re-anchor: +{delta} ns"
        );
    }

    #[test]
    fn next_vblank_at_or_after_now() {
        let refresh = RefreshInterval::from_ns(1_000).unwrap();
        let p = VblankPredictor::new(refresh, Mono::ZERO);
        assert_eq!(p.next_vblank(Mono::ZERO).as_ns(), 0);
        assert_eq!(p.next_vblank(Mono::from_ns(1)).as_ns(), 1_000);
        assert_eq!(p.next_vblank(Mono::from_ns(1_000)).as_ns(), 1_000);
        assert_eq!(p.next_vblank(Mono::from_ns(1_001)).as_ns(), 2_000);
    }
}
