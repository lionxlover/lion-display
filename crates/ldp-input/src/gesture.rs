//! Touchpad gesture state machines: swipe, pinch, hold, and the
//! two-finger scroll emulation.
//!
//! One [`GestureMachine`] consumes the touch contact stream of a
//! touchpad ([`crate::normalizer::TouchUpdate`], millimeter
//! coordinates) and emits recognized gestures as a flat event stream.
//! The classification policy, in classification order:
//!
//! * fingers below two: nothing (taps are the click policy's problem),
//! * **pending**: fingers are down but unclassified. Motion of any
//!   finger beyond `move_threshold` classifies the contact:
//!   two fingers moving without changing their spread scroll; two or
//!   more fingers whose spread changes pinch; three or more fingers
//!   moving in concert swipe,
//! * a pending contact that stays still for `hold_timeout` becomes a
//!   hold,
//! * ending semantics are uniform: lifting fingers ends a gesture
//!   cleanly (`cancelled = false`), adding fingers interrupts it
//!   (`cancelled = true`), and device drops cancel everything.
//!
//! Identities: each gesture instance gets a monotonically increasing
//! `id` (the wire groups a gesture's events); scroll instances are
//! internal — the seat turns them into pointer axis sequences.
//!
//! Determinism: the machine is driven purely by the frame timestamps
//! of the fed updates; no clocks are read.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

use crate::normalizer::{TouchContact, TouchUpdate};

/// Tunable recognition thresholds.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GestureConfig {
    /// How far (mm) any finger must travel from its origin before the
    /// contact classifies as motion.
    pub move_threshold_mm: f32,
    /// How much the inter-finger spread (mm) must change before the
    /// contact classifies as a pinch (and before a running scroll
    /// converts into a pinch).
    pub spread_threshold_mm: f32,
    /// How long (ns) fingers must stay within the move threshold
    /// before the contact classifies as a hold.
    pub hold_timeout_ns: u64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        GestureConfig {
            move_threshold_mm: 1.0,
            spread_threshold_mm: 2.0,
            hold_timeout_ns: 200_000_000,
        }
    }
}

/// One recognized gesture event.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GestureEvent {
    /// A swipe (3–4 fingers) started.
    SwipeBegin {
        /// Gesture instance.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// Swipe centroid delta since the last update (mm).
    SwipeUpdate {
        /// Gesture instance.
        id: u32,
        /// X delta.
        dx: f32,
        /// Y delta.
        dy: f32,
    },
    /// The swipe ended.
    SwipeEnd {
        /// Gesture instance.
        id: u32,
        /// Interrupted rather than completed.
        cancelled: bool,
    },
    /// A pinch (2–4 fingers) started.
    PinchBegin {
        /// Gesture instance.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// Pinch update: incremental scale (spread ratio) and centroid delta.
    PinchUpdate {
        /// Gesture instance.
        id: u32,
        /// Incremental scale factor (1 = unchanged).
        scale: f32,
        /// Centroid X delta (mm).
        dx: f32,
        /// Centroid Y delta (mm).
        dy: f32,
    },
    /// The pinch ended.
    PinchEnd {
        /// Gesture instance.
        id: u32,
        /// Interrupted rather than completed.
        cancelled: bool,
    },
    /// A hold (2–4 fingers, still) started.
    HoldBegin {
        /// Gesture instance.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// The hold ended.
    HoldEnd {
        /// Gesture instance.
        id: u32,
        /// Interrupted (motion) rather than completed (lift).
        cancelled: bool,
    },
    /// Two-finger scroll started (internal; the seat maps it to a
    /// finger-source axis sequence).
    ScrollBegin {
        /// Scroll instance.
        id: u32,
    },
    /// Scroll centroid delta since the last update (mm).
    ScrollUpdate {
        /// Scroll instance.
        id: u32,
        /// X delta.
        dx: f32,
        /// Y delta.
        dy: f32,
    },
    /// The scroll ended.
    ScrollEnd {
        /// Scroll instance.
        id: u32,
        /// Interrupted (spread grew into a pinch) rather than completed.
        cancelled: bool,
    },
}

#[derive(Clone, Debug)]
enum GState {
    Idle,
    Pending {
        fingers: u32,
        origin: Vec<(i32, f32, f32)>,
        start: Mono,
    },
    Swipe {
        id: u32,
        fingers: u32,
        centroid: (f32, f32),
    },
    Pinch {
        id: u32,
        fingers: u32,
        centroid: (f32, f32),
        spread: f32,
    },
    Hold {
        id: u32,
        fingers: u32,
    },
    Scroll {
        id: u32,
        centroid: (f32, f32),
        origin_spread: f32,
    },
}

/// The per-touchpad gesture recognizer.
#[derive(Clone, Debug)]
pub struct GestureMachine {
    cfg: GestureConfig,
    state: GState,
    next_id: u32,
    /// Origins captured when the current hold began (holds cancel on
    /// motion against their begin-time origins).
    hold_origin: Vec<(i32, f32, f32)>,
}

impl GestureMachine {
    /// A machine with the given thresholds.
    #[must_use]
    pub fn new(cfg: GestureConfig) -> GestureMachine {
        GestureMachine {
            cfg,
            state: GState::Idle,
            next_id: 1,
            hold_origin: Vec::new(),
        }
    }

    /// The active thresholds.
    #[must_use]
    pub fn config(&self) -> GestureConfig {
        self.cfg
    }

    /// The next gesture instance id (diagnostics).
    #[must_use]
    pub fn peek_next_id(&self) -> u32 {
        self.next_id
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Cancel any active gesture (device removal, `SYN_DROPPED`).
    pub fn reset(&mut self) -> Vec<GestureEvent> {
        self.lift(true)
    }

    /// Feed one touch frame; returns the recognized events.
    pub fn feed(&mut self, u: &TouchUpdate) -> Vec<GestureEvent> {
        if u.dropped {
            return self.reset();
        }
        let fingers = u.points.len() as u32;
        if fingers == 0 {
            return self.lift(false);
        }
        let state = core::mem::replace(&mut self.state, GState::Idle);
        match state {
            GState::Idle => {
                self.state = if fingers >= 2 {
                    GState::Pending {
                        fingers,
                        origin: snapshot(&u.points),
                        start: u.time,
                    }
                } else {
                    GState::Idle
                };
                Vec::new()
            }
            GState::Pending {
                fingers: prev,
                origin,
                start,
            } => self.feed_pending(u, fingers, prev, origin, start),
            GState::Swipe {
                id,
                fingers: active,
                centroid,
            } => self.feed_swipe(u, fingers, id, active, centroid),
            GState::Pinch {
                id,
                fingers: active,
                centroid,
                spread,
            } => self.feed_pinch(u, fingers, id, active, centroid, spread),
            GState::Hold {
                id,
                fingers: active,
            } => self.feed_hold(u, fingers, id, active),
            GState::Scroll {
                id,
                centroid,
                origin_spread,
            } => self.feed_scroll(u, fingers, id, centroid, origin_spread),
        }
    }

    /// Pending contacts either restart (finger-count change) or
    /// classify.
    fn feed_pending(
        &mut self,
        u: &TouchUpdate,
        fingers: u32,
        prev: u32,
        origin: Vec<(i32, f32, f32)>,
        start: Mono,
    ) -> Vec<GestureEvent> {
        if fingers != prev {
            // The contact changed during classification: restart from
            // the new contact set.
            self.state = if fingers >= 2 {
                GState::Pending {
                    fingers,
                    origin: snapshot(&u.points),
                    start: u.time,
                }
            } else {
                GState::Idle
            };
            return Vec::new();
        }
        self.classify(u, origin, start)
    }

    /// Swipe frames: centroid deltas, clean end on lift, interrupt on
    /// new fingers.
    fn feed_swipe(
        &mut self,
        u: &TouchUpdate,
        fingers: u32,
        id: u32,
        active: u32,
        centroid: (f32, f32),
    ) -> Vec<GestureEvent> {
        if fingers != active {
            let cancelled = fingers > active;
            return vec![GestureEvent::SwipeEnd { id, cancelled }];
        }
        let c = centroid_of(&u.points);
        let (dx, dy) = (c.0 - centroid.0, c.1 - centroid.1);
        let mut out = Vec::new();
        let moved = if dx != 0.0 || dy != 0.0 {
            out.push(GestureEvent::SwipeUpdate { id, dx, dy });
            c
        } else {
            centroid
        };
        self.state = GState::Swipe {
            id,
            fingers: active,
            centroid: moved,
        };
        out
    }

    /// Pinch frames: incremental scale (spread ratio) plus centroid
    /// deltas.
    fn feed_pinch(
        &mut self,
        u: &TouchUpdate,
        fingers: u32,
        id: u32,
        active: u32,
        centroid: (f32, f32),
        spread: f32,
    ) -> Vec<GestureEvent> {
        if fingers != active {
            let cancelled = fingers > active;
            return vec![GestureEvent::PinchEnd { id, cancelled }];
        }
        let c = centroid_of(&u.points);
        let s = spread_of(&u.points);
        let scale = if spread > 0.0 { s / spread } else { 1.0 };
        let (dx, dy) = (c.0 - centroid.0, c.1 - centroid.1);
        let mut out = Vec::new();
        let (nc, ns) = if (scale - 1.0).abs() > 1e-6 || dx != 0.0 || dy != 0.0 {
            out.push(GestureEvent::PinchUpdate { id, scale, dx, dy });
            (c, s)
        } else {
            (centroid, spread)
        };
        self.state = GState::Pinch {
            id,
            fingers: active,
            centroid: nc,
            spread: ns,
        };
        out
    }

    /// Hold frames: motion beyond the threshold against the
    /// begin-time origins cancels.
    fn feed_hold(
        &mut self,
        u: &TouchUpdate,
        fingers: u32,
        id: u32,
        active: u32,
    ) -> Vec<GestureEvent> {
        if fingers != active {
            let cancelled = fingers > active;
            return vec![GestureEvent::HoldEnd { id, cancelled }];
        }
        if let Some(moved) = moved_from(&u.points, &self.hold_origin) {
            if moved > self.cfg.move_threshold_mm {
                return vec![GestureEvent::HoldEnd {
                    id,
                    cancelled: true,
                }];
            }
        }
        self.state = GState::Hold {
            id,
            fingers: active,
        };
        Vec::new()
    }

    /// Scroll frames: centroid deltas; divergence converts to a pinch.
    fn feed_scroll(
        &mut self,
        u: &TouchUpdate,
        fingers: u32,
        id: u32,
        centroid: (f32, f32),
        origin_spread: f32,
    ) -> Vec<GestureEvent> {
        if fingers != 2 {
            let cancelled = fingers > 2;
            return vec![GestureEvent::ScrollEnd { id, cancelled }];
        }
        let c = centroid_of(&u.points);
        let s = spread_of(&u.points);
        if (s - origin_spread).abs() > self.cfg.spread_threshold_mm {
            // The fingers diverged: the scroll becomes a pinch.
            let pinch_id = self.fresh_id();
            self.state = GState::Pinch {
                id: pinch_id,
                fingers: 2,
                centroid: c,
                spread: s,
            };
            return vec![
                GestureEvent::ScrollEnd {
                    id,
                    cancelled: true,
                },
                GestureEvent::PinchBegin {
                    id: pinch_id,
                    fingers: 2,
                },
            ];
        }
        let (dx, dy) = (c.0 - centroid.0, c.1 - centroid.1);
        let mut out = Vec::new();
        let moved = if dx != 0.0 || dy != 0.0 {
            out.push(GestureEvent::ScrollUpdate { id, dx, dy });
            c
        } else {
            centroid
        };
        self.state = GState::Scroll {
            id,
            centroid: moved,
            origin_spread,
        };
        out
    }

    /// Classify a pending contact: motion, hold, or still pending.
    fn classify(
        &mut self,
        u: &TouchUpdate,
        origin: Vec<(i32, f32, f32)>,
        start: Mono,
    ) -> Vec<GestureEvent> {
        let moved = moved_from(&u.points, &origin).unwrap_or(f32::INFINITY);
        let hold_elapsed = u.time.as_ns().saturating_sub(start.as_ns());
        if moved <= self.cfg.move_threshold_mm {
            if hold_elapsed >= self.cfg.hold_timeout_ns {
                let id = self.fresh_id();
                let fingers = u.points.len() as u32;
                self.hold_origin = origin;
                self.state = GState::Hold { id, fingers };
                return vec![GestureEvent::HoldBegin { id, fingers }];
            }
            self.state = GState::Pending {
                fingers: u.points.len() as u32,
                origin,
                start,
            };
            return Vec::new();
        }
        // Motion classified. Spread change decides pinch vs. parallel.
        let spread_now = spread_of(&u.points);
        let spread_then = spread_snapshot(&origin);
        let spread_changed = (spread_now - spread_then).abs() > self.cfg.spread_threshold_mm;
        let fingers = u.points.len() as u32;
        let c = centroid_of(&u.points);
        if fingers == 2 && !spread_changed {
            let id = self.fresh_id();
            self.state = GState::Scroll {
                id,
                centroid: c,
                origin_spread: spread_then,
            };
            vec![GestureEvent::ScrollBegin { id }]
        } else if spread_changed && fingers >= 2 {
            let id = self.fresh_id();
            self.state = GState::Pinch {
                id,
                fingers,
                centroid: c,
                spread: spread_now,
            };
            vec![GestureEvent::PinchBegin { id, fingers }]
        } else if fingers >= 3 {
            let id = self.fresh_id();
            self.state = GState::Swipe {
                id,
                fingers,
                centroid: c,
            };
            vec![GestureEvent::SwipeBegin { id, fingers }]
        } else {
            // Two fingers with sub-threshold spread change: keep
            // pending (micro-jitter must not classify a pinch).
            self.state = GState::Pending {
                fingers,
                origin,
                start,
            };
            Vec::new()
        }
    }

    /// Every finger left (or a drop forced the equivalent): end any
    /// active gesture.
    fn lift(&mut self, cancelled: bool) -> Vec<GestureEvent> {
        match core::mem::replace(&mut self.state, GState::Idle) {
            GState::Swipe { id, .. } => vec![GestureEvent::SwipeEnd { id, cancelled }],
            GState::Pinch { id, .. } => vec![GestureEvent::PinchEnd { id, cancelled }],
            GState::Hold { id, .. } => vec![GestureEvent::HoldEnd { id, cancelled }],
            GState::Scroll { id, .. } => vec![GestureEvent::ScrollEnd { id, cancelled }],
            GState::Idle | GState::Pending { .. } => Vec::new(),
        }
    }
}

#[must_use]
fn snapshot(points: &[TouchContact]) -> Vec<(i32, f32, f32)> {
    points.iter().map(|p| (p.id, p.x_mm, p.y_mm)).collect()
}

#[must_use]
fn centroid_of(points: &[TouchContact]) -> (f32, f32) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let n = points.len() as f32;
    (
        points.iter().map(|p| p.x_mm).sum::<f32>() / n,
        points.iter().map(|p| p.y_mm).sum::<f32>() / n,
    )
}

#[must_use]
fn spread_of(points: &[TouchContact]) -> f32 {
    // Mean pairwise distance (the two-finger case is the distance).
    if points.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0f32;
    let mut pairs = 0usize;
    for i in 0..points.len() {
        for j in (i + 1)..points.len() {
            let dx = points[i].x_mm - points[j].x_mm;
            let dy = points[i].y_mm - points[j].y_mm;
            total += (dx * dx + dy * dy).sqrt();
            pairs += 1;
        }
    }
    if pairs == 0 {
        0.0
    } else {
        total / pairs as f32
    }
}

#[must_use]
fn spread_snapshot(origin: &[(i32, f32, f32)]) -> f32 {
    if origin.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0f32;
    let mut pairs = 0usize;
    for i in 0..origin.len() {
        for j in (i + 1)..origin.len() {
            let dx = origin[i].1 - origin[j].1;
            let dy = origin[i].2 - origin[j].2;
            total += (dx * dx + dy * dy).sqrt();
            pairs += 1;
        }
    }
    if pairs == 0 {
        0.0
    } else {
        total / pairs as f32
    }
}

/// Maximum displacement (mm) of any current point from its origin.
#[must_use]
fn moved_from(points: &[TouchContact], origin: &[(i32, f32, f32)]) -> Option<f32> {
    let mut max = 0.0f32;
    let mut any = false;
    for p in points {
        if let Some((_, ox, oy)) = origin.iter().find(|(id, _, _)| *id == p.id) {
            let d = ((p.x_mm - ox).powi(2) + (p.y_mm - oy).powi(2)).sqrt();
            max = max.max(d);
            any = true;
        }
    }
    any.then_some(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: i32, x_mm: f32, y_mm: f32) -> TouchContact {
        TouchContact {
            id,
            x: 0.0,
            y: 0.0,
            x_mm,
            y_mm,
            pressure: None,
            touch_major: None,
            touch_minor: None,
            orientation: None,
            tool: None,
        }
    }

    fn frame(t_ms: u64, points: &[TouchContact]) -> TouchUpdate {
        TouchUpdate {
            time: Mono::from_ms(t_ms),
            points: points.to_vec(),
            began: Vec::new(),
            ended: Vec::new(),
            dropped: false,
        }
    }

    fn machine() -> GestureMachine {
        GestureMachine::new(GestureConfig::default())
    }

    #[test]
    fn three_finger_swipe() {
        let mut g = machine();
        // Three fingers down at t=0.
        assert!(g
            .feed(&frame(
                0,
                &[
                    contact(1, 10.0, 10.0),
                    contact(2, 12.0, 10.0),
                    contact(3, 14.0, 10.0)
                ]
            ))
            .is_empty());
        // Move all three right by 3mm (beyond the 1mm threshold).
        let out = g.feed(&frame(
            16,
            &[
                contact(1, 13.0, 10.0),
                contact(2, 15.0, 10.0),
                contact(3, 17.0, 10.0),
            ],
        ));
        assert_eq!(out, vec![GestureEvent::SwipeBegin { id: 1, fingers: 3 }]);
        // Continue moving: updates carry centroid deltas.
        let out = g.feed(&frame(
            32,
            &[
                contact(1, 15.0, 10.0),
                contact(2, 17.0, 10.0),
                contact(3, 19.0, 10.0),
            ],
        ));
        assert_eq!(
            out,
            vec![GestureEvent::SwipeUpdate {
                id: 1,
                dx: 2.0,
                dy: 0.0
            }]
        );
        // All fingers lift: clean end.
        let out = g.feed(&frame(48, &[]));
        assert_eq!(
            out,
            vec![GestureEvent::SwipeEnd {
                id: 1,
                cancelled: false
            }]
        );
    }

    #[test]
    fn two_finger_scroll() {
        let mut g = machine();
        assert!(g
            .feed(&frame(0, &[contact(1, 20.0, 20.0), contact(2, 24.0, 20.0)]))
            .is_empty());
        // Both fingers move down 2mm together: parallel → scroll.
        let out = g.feed(&frame(
            16,
            &[contact(1, 20.0, 22.0), contact(2, 24.0, 22.0)],
        ));
        assert_eq!(out, vec![GestureEvent::ScrollBegin { id: 1 }]);
        let out = g.feed(&frame(
            32,
            &[contact(1, 20.0, 25.0), contact(2, 24.0, 25.0)],
        ));
        assert_eq!(
            out,
            vec![GestureEvent::ScrollUpdate {
                id: 1,
                dx: 0.0,
                dy: 3.0
            }]
        );
        let out = g.feed(&frame(48, &[]));
        assert_eq!(
            out,
            vec![GestureEvent::ScrollEnd {
                id: 1,
                cancelled: false
            }]
        );
    }

    #[test]
    fn two_finger_pinch() {
        let mut g = machine();
        assert!(g
            .feed(&frame(0, &[contact(1, 20.0, 20.0), contact(2, 24.0, 20.0)]))
            .is_empty());
        // Fingers diverge: spread 4mm → 14mm.
        let out = g.feed(&frame(
            16,
            &[contact(1, 15.0, 20.0), contact(2, 29.0, 20.0)],
        ));
        assert_eq!(out, vec![GestureEvent::PinchBegin { id: 1, fingers: 2 }]);
        // Diverge further: spread 14 → 16 (scale = 16/14).
        let out = g.feed(&frame(
            32,
            &[contact(1, 14.0, 20.0), contact(2, 30.0, 20.0)],
        ));
        match out[0] {
            GestureEvent::PinchUpdate { id, scale, dx, dy } => {
                assert_eq!(id, 1);
                assert!((scale - 16.0 / 14.0).abs() < 1e-5);
                assert!((dx - 0.0).abs() < 1e-6);
                assert!((dy - 0.0).abs() < 1e-6);
            }
            ref other => panic!("unexpected {other:?}"),
        }
        let out = g.feed(&frame(48, &[]));
        assert_eq!(
            out,
            vec![GestureEvent::PinchEnd {
                id: 1,
                cancelled: false
            }]
        );
    }

    #[test]
    fn hold_after_timeout_and_motion_cancel() {
        let mut g = machine();
        assert!(g
            .feed(&frame(0, &[contact(1, 10.0, 10.0), contact(2, 12.0, 10.0)]))
            .is_empty());
        // 100 ms: below the 200 ms timeout, still pending.
        assert!(g
            .feed(&frame(
                100,
                &[contact(1, 10.0, 10.0), contact(2, 12.0, 10.0)]
            ))
            .is_empty());
        // 250 ms: hold begins.
        let out = g.feed(&frame(
            250,
            &[contact(1, 10.0, 10.0), contact(2, 12.0, 10.0)],
        ));
        assert_eq!(out, vec![GestureEvent::HoldBegin { id: 1, fingers: 2 }]);
        // Fingers move beyond the threshold: cancelled hold.
        let out = g.feed(&frame(
            300,
            &[contact(1, 12.0, 10.0), contact(2, 14.0, 10.0)],
        ));
        assert_eq!(
            out,
            vec![GestureEvent::HoldEnd {
                id: 1,
                cancelled: true
            }]
        );
    }

    #[test]
    fn adding_fingers_interrupts_swipe() {
        let mut g = machine();
        g.feed(&frame(
            0,
            &[
                contact(1, 10.0, 10.0),
                contact(2, 12.0, 10.0),
                contact(3, 14.0, 10.0),
            ],
        ));
        g.feed(&frame(
            16,
            &[
                contact(1, 13.0, 10.0),
                contact(2, 15.0, 10.0),
                contact(3, 17.0, 10.0),
            ],
        ));
        // A fourth finger lands mid-swipe: interrupted.
        let out = g.feed(&frame(
            24,
            &[
                contact(1, 13.0, 10.0),
                contact(2, 15.0, 10.0),
                contact(3, 17.0, 10.0),
                contact(4, 20.0, 10.0),
            ],
        ));
        assert_eq!(
            out,
            vec![GestureEvent::SwipeEnd {
                id: 1,
                cancelled: true
            }]
        );
    }

    #[test]
    fn dropped_frame_cancels() {
        let mut g = machine();
        g.feed(&frame(
            0,
            &[
                contact(1, 10.0, 10.0),
                contact(2, 12.0, 10.0),
                contact(3, 14.0, 10.0),
            ],
        ));
        g.feed(&frame(
            16,
            &[
                contact(1, 13.0, 10.0),
                contact(2, 15.0, 10.0),
                contact(3, 17.0, 10.0),
            ],
        ));
        let mut dropped = frame(32, &[]);
        dropped.dropped = true;
        let out = g.feed(&dropped);
        assert_eq!(
            out,
            vec![GestureEvent::SwipeEnd {
                id: 1,
                cancelled: true
            }]
        );
    }

    #[test]
    fn scroll_converts_to_pinch_on_spread() {
        let mut g = machine();
        g.feed(&frame(0, &[contact(1, 20.0, 20.0), contact(2, 24.0, 20.0)]));
        g.feed(&frame(
            16,
            &[contact(1, 20.0, 22.0), contact(2, 24.0, 22.0)],
        ));
        // Now the fingers diverge well beyond the spread threshold.
        let out = g.feed(&frame(
            32,
            &[contact(1, 10.0, 22.0), contact(2, 34.0, 22.0)],
        ));
        assert_eq!(
            out,
            vec![
                GestureEvent::ScrollEnd {
                    id: 1,
                    cancelled: true
                },
                GestureEvent::PinchBegin { id: 2, fingers: 2 },
            ]
        );
    }

    #[test]
    fn single_finger_and_quick_tap_emit_nothing() {
        let mut g = machine();
        assert!(g.feed(&frame(0, &[contact(1, 10.0, 10.0)])).is_empty());
        // Two fingers for 50 ms then lift: unclassified → silence.
        assert!(g
            .feed(&frame(
                16,
                &[contact(1, 10.0, 10.0), contact(2, 12.0, 10.0)]
            ))
            .is_empty());
        assert!(g.feed(&frame(50, &[])).is_empty());
    }
}
