//! The mock scanout timeline — deterministic vblank arithmetic.
//!
//! Doctrine (matching Phase 7/8): **no clock reads**. The device owns an
//! injected [`Mono`] clock the test advances; every event timestamp is
//! computed from that clock, so recordings are byte-reproducible.
//!
//! One [`CrtcTimeline`] per active CRTC. The model:
//!
//! * **VRR off** — vblanks sit on the strict nominal grid
//!   `last + k · nominal`; a flip submitted at `t` lands at the first
//!   grid point ≥ `t`.
//! * **VRR on** — a flip lands at `clamp(max(t, last + min), last + min,
//!   last + max)`: the panel can re-trigger no earlier than `min` after
//!   the previous scanout and stretches at most to `max`. With no flip
//!   pending the panel idles at `max` period.
//! * **In-fences** — a flip completes only when both its vblank time has
//!   arrived *and* the acquire fence is ready; the completion timestamp
//!   is the later of the two.
//! * **Chained flips** — a blocking commit behind a pending flip is
//!   sequenced off the pending flip's completion (the kernel queues at
//!   most one; the mock chains deterministically).

#![forbid(unsafe_code)]

use crate::events::{DeviceEvent, PageFlipEvent, PageFlipFlags, VblankEvent};
use crate::ids::CrtcId;
use ldp_core::time::Mono;

/// A flip waiting for its vblank.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingFlip {
    /// The vblank time the flip targets.
    pub target: Mono,
    /// When the acquire fence becomes ready (`None` = no fence).
    pub fence_ready: Option<Mono>,
    /// Whether a page-flip event should be emitted.
    pub wants_event: bool,
    /// Out-fence token minted for this flip (delivered with the event).
    pub out_fence: Option<u64>,
}

/// One CRTC's virtual scanout clock.
#[derive(Clone, Debug)]
pub(crate) struct CrtcTimeline {
    /// Nominal frame period (the mode's period).
    nominal_ns: u64,
    /// The VRR window `(min, max)` in ns when enabled.
    vrr_window: Option<(u64, u64)>,
    /// Time of the last emitted vblank.
    last: Mono,
    /// Every vblank increments (flip or not).
    vblank_seq: u64,
    /// Flips completed so far.
    flip_seq: u64,
    /// The queued flips; index 0 is next.
    pending: Vec<PendingFlip>,
}

impl CrtcTimeline {
    /// A timeline starting at `origin` with the given nominal period.
    pub(crate) fn new(origin: Mono, nominal_ns: u64) -> Self {
        Self {
            nominal_ns,
            vrr_window: None,
            last: origin,
            vblank_seq: 0,
            flip_seq: 0,
            pending: Vec::new(),
        }
    }

    /// Set the nominal period (mode change).
    pub(crate) fn set_nominal(&mut self, ns: u64) {
        self.nominal_ns = ns;
    }

    /// Enable/disable VRR with its `(min, max)` window.
    pub(crate) fn set_vrr(&mut self, window: Option<(u64, u64)>) {
        self.vrr_window = window;
    }

    /// Re-anchor the grid at `now` — the panel-self-refresh rescan
    /// (Phase 35). While a connector's PSR property is on, the
    /// timeline is frozen (the panel holds its GRAM; no vblanks, no
    /// flips); on release — the property write or the page flip that
    /// implies it — the display engine restarts the scan at `now`,
    /// and the *next* flip submitted lands a full nominal period
    /// later: the honest rescan cost, one refresh interval, never
    /// free.
    pub(crate) fn reanchor(&mut self, now: Mono) {
        self.last = now;
    }

    /// The current nominal period.
    pub(crate) fn nominal(&self) -> u64 {
        self.nominal_ns
    }

    /// The effective "last vblank" for sequencing: the last emitted
    /// vblank, or the completion of the final queued flip.
    fn sequencing_anchor(&self) -> Mono {
        match self.pending.last() {
            Some(flip) => flip.target,
            None => self.last,
        }
    }

    /// Queue a flip submitted at `now`.
    ///
    /// Returns the expected completion time (pre-fence).
    pub(crate) fn submit_flip(
        &mut self,
        now: Mono,
        wants_event: bool,
        out_fence: Option<u64>,
    ) -> Mono {
        let anchor = self.sequencing_anchor();
        let target = match self.vrr_window {
            None => {
                // Strict nominal grid: first point strictly after `now`
                // (a flip at exactly a vblank lands on the next one).
                let elapsed = now.as_ns().saturating_sub(anchor.as_ns());
                let k = elapsed / self.nominal_ns + 1;
                anchor.saturating_add_ns(k * self.nominal_ns)
            }
            Some((min, max)) => {
                let earliest = anchor.saturating_add_ns(min).max(now);
                let latest = anchor.saturating_add_ns(max);
                if earliest > latest {
                    latest
                } else {
                    earliest
                }
            }
        };
        self.pending.push(PendingFlip {
            target,
            fence_ready: None,
            wants_event,
            out_fence,
        });
        target
    }

    /// Attach an acquire-fence readiness time to the newest pending flip.
    pub(crate) fn set_fence_ready(&mut self, ready: Mono) {
        if let Some(flip) = self.pending.last_mut() {
            flip.fence_ready = Some(ready);
        }
    }

    /// Advance past every tick due at or before `now`, emitting events.
    ///
    /// A queued flip completes only when `target <= now` *and* its fence
    /// is ready; a fence-blocked flip holds the queue (later flips stay
    /// queued behind it, matching scanout order).
    pub(crate) fn advance_to(&mut self, now: Mono, crtc: CrtcId) -> Vec<DeviceEvent> {
        let mut out = Vec::new();
        loop {
            let Some(front) = self.pending.first().copied() else {
                // Idle panel: bare vblanks at nominal (VRR off) or max
                // (VRR on).
                let interval = match self.vrr_window {
                    Some((_, max)) => max,
                    None => self.nominal_ns,
                };
                let next = self.last.saturating_add_ns(interval);
                if next > now {
                    break;
                }
                self.last = next;
                self.vblank_seq += 1;
                out.push(DeviceEvent::Vblank(VblankEvent {
                    crtc,
                    sequence: self.vblank_seq,
                    timestamp: next,
                }));
                continue;
            };
            let ready_at = front.target.max(front.fence_ready.unwrap_or(Mono::ZERO));
            if ready_at > now {
                break;
            }
            // Complete the flip.
            self.pending.remove(0);
            self.last = ready_at;
            self.vblank_seq += 1;
            self.flip_seq += 1;
            if front.wants_event {
                out.push(DeviceEvent::PageFlip(PageFlipEvent {
                    crtc,
                    sequence: self.flip_seq,
                    timestamp: ready_at,
                    flags: PageFlipFlags::NONE,
                    out_fence: front.out_fence.map(crate::events::OutFence::Token),
                }));
            } else {
                out.push(DeviceEvent::Vblank(VblankEvent {
                    crtc,
                    sequence: self.vblank_seq,
                    timestamp: ready_at,
                }));
            }
        }
        out
    }

    /// Vblanks emitted so far (test introspection).
    pub(crate) fn vblank_seq(&self) -> u64 {
        self.vblank_seq
    }

    /// Flips completed so far (test introspection).
    pub(crate) fn flip_seq(&self) -> u64 {
        self.flip_seq
    }

    /// Queued flips (test introspection).
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Time the device clock must reach for the next event (for tests
    /// that want to step exactly): the head flip's completion, or the
    /// next idle vblank. A live timeline always has a next event.
    pub(crate) fn next_event_at(&self) -> Mono {
        if let Some(front) = self.pending.first() {
            front.target.max(front.fence_ready.unwrap_or(Mono::ZERO))
        } else {
            let interval = match self.vrr_window {
                Some((_, max)) => max,
                None => self.nominal_ns,
            };
            self.last.saturating_add_ns(interval)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: u64 = 16_666_666;

    #[test]
    fn nominal_grid_flips() {
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        // Submit mid-frame: lands at the next grid point.
        let at = t.submit_flip(Mono::from_ns(1_000_000), true, None);
        assert_eq!(at.as_ns(), NS);
        let events = t.advance_to(Mono::from_ns(NS), CrtcId::new(1).unwrap());
        assert_eq!(events.len(), 1);
        match &events[0] {
            DeviceEvent::PageFlip(f) => {
                assert_eq!(f.timestamp.as_ns(), NS);
                assert_eq!(f.sequence, 1);
            }
            _ => panic!("expected flip"),
        }
        // Next flip lands on the following grid point even when submitted
        // exactly at the previous vblank.
        let at2 = t.submit_flip(Mono::from_ns(NS), true, None);
        assert_eq!(at2.as_ns(), 2 * NS);
    }

    #[test]
    fn idle_vblanks_tick_at_nominal() {
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        let events = t.advance_to(Mono::from_ns(3 * NS), CrtcId::new(1).unwrap());
        assert_eq!(events.len(), 3);
        for (i, ev) in events.iter().enumerate() {
            match ev {
                DeviceEvent::Vblank(v) => {
                    assert_eq!(v.sequence, i as u64 + 1);
                    assert_eq!(v.timestamp.as_ns(), (i as u64 + 1) * NS);
                }
                _ => panic!("expected vblank"),
            }
        }
    }

    #[test]
    fn vrr_early_flip_and_idle_stretch() {
        const MIN: u64 = 6_944_444; // 144 Hz
        const MAX: u64 = 20_833_333; // 48 Hz
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        t.set_vrr(Some((MIN, MAX)));
        // Flip submitted mid-frame completes at max(now, last+min).
        let at = t.submit_flip(Mono::from_ns(10_000_000), true, None);
        assert_eq!(at.as_ns(), 10_000_000); // > MIN, < MAX after last=0
        t.advance_to(Mono::from_ns(10_000_000), CrtcId::new(1).unwrap());
        // Early second flip: last=10ms, min after that.
        let at2 = t.submit_flip(Mono::from_ns(11_000_000), true, None);
        assert_eq!(at2.as_ns(), 10_000_000 + MIN);
        // Idle: next bare vblank stretches to max.
        t.advance_to(Mono::from_ns(10_000_000 + MIN), CrtcId::new(1).unwrap());
        assert_eq!(t.next_event_at().as_ns(), 10_000_000 + MIN + MAX);
    }

    #[test]
    fn vrr_flip_too_early_waits_min() {
        const MIN: u64 = 6_944_444;
        const MAX: u64 = 20_833_333;
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        t.set_vrr(Some((MIN, MAX)));
        // Submitted immediately after a vblank: earliest is last+min.
        let at = t.submit_flip(Mono::from_ns(1), true, None);
        assert_eq!(at.as_ns(), MIN);
    }

    #[test]
    fn fence_holds_the_queue() {
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        let _ = t.submit_flip(Mono::from_ns(100), true, None);
        t.set_fence_ready(Mono::from_ns(2 * NS));
        // Past the vblank but fence not ready: nothing fires.
        assert!(t
            .advance_to(Mono::from_ns(NS), CrtcId::new(1).unwrap())
            .is_empty());
        // At fence readiness the flip completes with the fence timestamp.
        let events = t.advance_to(Mono::from_ns(2 * NS), CrtcId::new(1).unwrap());
        match events.first() {
            Some(DeviceEvent::PageFlip(f)) => assert_eq!(f.timestamp.as_ns(), 2 * NS),
            _ => panic!("expected flip"),
        }
    }

    #[test]
    fn chained_flips_sequence_off_completion() {
        let mut t = CrtcTimeline::new(Mono::ZERO, NS);
        let a = t.submit_flip(Mono::from_ns(100), true, None);
        // Blocking-style second flip chains off the first's completion.
        let b = t.submit_flip(Mono::from_ns(a.as_ns()), true, None);
        assert_eq!(b.as_ns(), 2 * NS);
        assert_eq!(t.pending_count(), 2);
    }
}
