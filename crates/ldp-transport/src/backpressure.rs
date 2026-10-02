//! Backpressure: writer hooks and watermarks.
//!
//! `docs/architecture.md` §6: writes are nonblocking; a full socket
//! parks the *client's* event queue — the client is wedging itself,
//! never the server. The transport's half of that contract is a hard
//! memory bound ([`BackpressureConfig::ceiling`], from
//! `Limits::event_queue_bytes`, 2 MiB by default) plus edge-triggered
//! [`WriterHooks`] so the server's event loop can coalesce or drop
//! instead of buffering.
//!
//! Events (in order on a typical slow-peer cycle):
//!
//! 1. `on_congestion(pending)` — the socket refused bytes; a tail is
//!    now queued locally,
//! 2. `on_drain(pending)` — the tail fell below
//!    [`BackpressureConfig::low_water`],
//! 3. `on_recovery()` — the tail fully drained,
//! 4. `on_overflow(attempted, ceiling)` — a send was *rejected* because
//!    it would exceed the ceiling (the caller decides: coalesce, drop,
//!    or fail the connection with `limit_exceeded`).

#![forbid(unsafe_code)]

use ldp_core::limits::Limits;
use std::cell::RefCell;

/// Observer hooks invoked on [`FramedWriter`](crate::writer::FramedWriter)
/// state transitions. All methods have empty defaults; implementations
/// must be cheap (they run on the I/O path).
pub trait WriterHooks {
    /// The socket refused more bytes (partial write or EAGAIN); the
    /// writer is now holding `pending_bytes` for this peer.
    fn on_congestion(&mut self, pending_bytes: u64) {
        let _ = pending_bytes;
    }

    /// Queued bytes fell below the low watermark while draining.
    fn on_drain(&mut self, pending_bytes: u64) {
        let _ = pending_bytes;
    }

    /// The queue fully drained after congestion.
    fn on_recovery(&mut self) {}

    /// A send was rejected: queueing `attempted` bytes would exceed
    /// `ceiling`. The message was *not* consumed.
    fn on_overflow(&mut self, attempted: u64, ceiling: u64) {
        let _ = (attempted, ceiling);
    }
}

/// The do-nothing hooks (the default observer).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHooks;

impl WriterHooks for NoHooks {}

/// Backpressure configuration for one writer.
///
/// `ceiling` bounds the locally queued tail: a send that would push the
/// queue past it fails with `limit_exceeded` (after `on_overflow`), so
/// memory for one slow peer is bounded by exactly this number plus the
/// in-flight syscall buffers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BackpressureConfig {
    /// Maximum bytes the writer may queue for one peer
    /// (`Limits::event_queue_bytes`).
    pub ceiling: u64,
    /// `on_drain` fires when pending bytes fall below this value
    /// (default: `ceiling / 4`).
    pub low_water: u64,
}

impl BackpressureConfig {
    /// Derive the configuration from the negotiated limits.
    ///
    /// The ceiling is `max(event_queue_bytes, message_bytes)`: a
    /// message already passed the per-message ceiling is *legal* and
    /// the writer cannot partially refuse it — one legal message is the
    /// irreducible unit of parking. Under the default limits the two
    /// agree in spirit (1 MiB message vs 2 MiB queue); under
    /// `large_messages` a parked 64 MiB message bounds the worst case
    /// explicitly.
    #[must_use]
    pub const fn from_limits(limits: &Limits) -> BackpressureConfig {
        BackpressureConfig {
            ceiling: if limits.event_queue_bytes > limits.message_bytes {
                limits.event_queue_bytes
            } else {
                limits.message_bytes
            },
            low_water: limits.event_queue_bytes / 4,
        }
    }
}

/// One recorded hook event (test and diagnostics helper).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HookEvent {
    /// See [`WriterHooks::on_congestion`].
    Congestion(u64),
    /// See [`WriterHooks::on_drain`].
    Drain(u64),
    /// See [`WriterHooks::on_recovery`].
    Recovery,
    /// See [`WriterHooks::on_overflow`].
    Overflow(u64, u64),
}

/// A [`WriterHooks`] implementation that records every event in order —
/// the assertion vehicle for the backpressure exit criteria.
#[derive(Debug, Default)]
pub struct HookRecorder {
    events: RefCell<Vec<HookEvent>>,
}

impl HookRecorder {
    /// A fresh, empty recorder.
    #[must_use]
    pub fn new() -> HookRecorder {
        HookRecorder::default()
    }

    /// The events recorded so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<HookEvent> {
        self.events.borrow().clone()
    }

    /// Number of events recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.borrow().len()
    }

    /// Whether any event was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.borrow().is_empty()
    }
}

impl WriterHooks for HookRecorder {
    fn on_congestion(&mut self, pending_bytes: u64) {
        self.events
            .borrow_mut()
            .push(HookEvent::Congestion(pending_bytes));
    }

    fn on_drain(&mut self, pending_bytes: u64) {
        self.events
            .borrow_mut()
            .push(HookEvent::Drain(pending_bytes));
    }

    fn on_recovery(&mut self) {
        self.events.borrow_mut().push(HookEvent::Recovery);
    }

    fn on_overflow(&mut self, attempted: u64, ceiling: u64) {
        self.events
            .borrow_mut()
            .push(HookEvent::Overflow(attempted, ceiling));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_derives_from_limits() {
        let cfg = BackpressureConfig::from_limits(&Limits::DEFAULT);
        assert_eq!(cfg.ceiling, 2 * 1024 * 1024);
        assert_eq!(cfg.low_water, 512 * 1024);
        // A legal message larger than the event queue is queueable: the
        // ceiling floors at the message ceiling (Phase 4 interop fix).
        let cfg = BackpressureConfig::from_limits(&Limits::LARGE_MESSAGES);
        assert_eq!(cfg.ceiling, 64 * 1024 * 1024);
        assert_eq!(cfg.low_water, 512 * 1024);
    }

    #[test]
    fn recorder_records_in_order() {
        let mut r = HookRecorder::new();
        assert!(r.is_empty());
        WriterHooks::on_congestion(&mut r, 100);
        WriterHooks::on_drain(&mut r, 40);
        WriterHooks::on_recovery(&mut r);
        WriterHooks::on_overflow(&mut r, 900, 800);
        assert_eq!(
            r.events(),
            vec![
                HookEvent::Congestion(100),
                HookEvent::Drain(40),
                HookEvent::Recovery,
                HookEvent::Overflow(900, 800),
            ]
        );
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn no_hooks_is_silent_and_zero_cost() {
        let mut h = NoHooks;
        WriterHooks::on_congestion(&mut h, 1);
        WriterHooks::on_drain(&mut h, 1);
        WriterHooks::on_recovery(&mut h);
        WriterHooks::on_overflow(&mut h, 1, 1);
        assert_eq!(size_of::<NoHooks>(), 0);
    }
}
