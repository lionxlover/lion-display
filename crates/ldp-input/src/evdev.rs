//! The evdev wire codec: 24-byte `input_event` records and
//! `SYN_REPORT` framing.
//!
//! The kernel ABI on LP64 is exactly:
//!
//! ```text
//! struct input_event {
//!     struct timeval time;   // 16 bytes: i64 sec + i64 usec
//!     __u16 type;            // 2 bytes
//!     __u16 code;            // 2 bytes
//!     __s32 value;           // 4 bytes
//! };                         // 24 bytes total, no padding
//! ```
//!
//! Decoding cannot fail structurally (every bit pattern is a valid
//! event), so the codec returns values, never errors; the stream
//! decoder simply buffers trailing partial records until more bytes
//! arrive. Encoding exists for the test corpus and for the future
//! injection path (which the security phase gates).
//!
//! Framing: events accumulate until `SYN_REPORT`, which closes one
//! [`DeviceFrame`] — the atomic unit every downstream consumer
//! (normalizer, gesture machines) operates on. `SYN_DROPPED` marks the
//! *next* frame as a resynchronization point; `SYN_MT_REPORT`
//! (slotless protocol A) is surfaced as a flag rather than silently
//! misparsed, and the classifier refuses protocol-A multitouch.
//!
//! Timestamps: the kernel fills `time` from its monotonic-ish clock;
//! this crate never interprets the epoch, it only converts µs to
//! [`Mono`]. Callers feeding synthetic traces control time absolutely,
//! which is what keeps the golden corpus byte-reproducible.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

use crate::codes::{ev, syn};

/// One decoded kernel `input_event`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawEvent {
    /// Event timestamp, microseconds since the device's epoch.
    pub time_us: u64,
    /// Event type (`EV_*`).
    pub ev_type: u16,
    /// Event code (a `KEY_*`/`REL_*`/`ABS_*`/... value).
    pub code: u16,
    /// Event payload (state, delta, or axis value).
    pub value: i32,
}

impl RawEvent {
    /// Construct an event.
    pub const fn new(time_us: u64, ev_type: u16, code: u16, value: i32) -> RawEvent {
        RawEvent {
            time_us,
            ev_type,
            code,
            value,
        }
    }

    /// The event time as a monotonic instant.
    #[must_use]
    pub const fn mono(&self) -> Mono {
        Mono::from_ns(self.time_us.saturating_mul(1000))
    }

    /// Encode to the 24-byte wire record.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&((self.time_us / 1_000_000) as i64).to_le_bytes());
        b[8..16].copy_from_slice(&((self.time_us % 1_000_000) as i64).to_le_bytes());
        b[16..18].copy_from_slice(&self.ev_type.to_le_bytes());
        b[18..20].copy_from_slice(&self.code.to_le_bytes());
        b[20..24].copy_from_slice(&self.value.to_le_bytes());
        b
    }

    /// Whether this is the frame-closing `SYN_REPORT`.
    #[must_use]
    pub const fn is_syn_report(&self) -> bool {
        self.ev_type == ev::SYN && self.code == syn::REPORT
    }
}

/// Decode one 24-byte record.
///
/// The timeval's two `i64` words are folded: `sec * 1_000_000 + usec`
/// into [`RawEvent::time_us`]. Negative seconds (pre-1970 clocks) are
/// rejected by clamping to zero — evdev timestamps are monotonic in
/// practice, and a negative value indicates a broken driver whose
/// events should not silently reorder.
///
/// # Panics
/// Never: the argument type pins the record width.
#[must_use]
pub fn decode_record(b: &[u8; 24]) -> RawEvent {
    let sec = i64::from_le_bytes(b[0..8].try_into().expect("8 bytes"));
    let usec = i64::from_le_bytes(b[8..16].try_into().expect("8 bytes"));
    let time_us = if sec <= 0 {
        usec.max(0) as u64
    } else {
        (sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add(usec.max(0) as u64)
    };
    RawEvent {
        time_us,
        ev_type: u16::from_le_bytes(b[16..18].try_into().expect("2 bytes")),
        code: u16::from_le_bytes(b[18..20].try_into().expect("2 bytes")),
        value: i32::from_le_bytes(b[20..24].try_into().expect("4 bytes")),
    }
}

/// A byte-stream decoder that tolerates partial records.
///
/// `read(2)` on an evdev fd returns whole records in practice, but the
/// transport doctrine of this project is that framing is never assumed:
/// feed arbitrary chunks, receive the events they complete.
#[derive(Clone, Debug, Default)]
pub struct StreamDecoder {
    /// Trailing bytes of an incomplete record.
    partial: Vec<u8>,
}

impl StreamDecoder {
    /// A decoder with nothing buffered.
    #[must_use]
    pub fn new() -> StreamDecoder {
        StreamDecoder::default()
    }

    /// Feed a byte chunk; returns the records it completed.
    ///
    /// # Panics
    /// Never: slicing is on 24-byte boundaries guarded by length
    /// checks.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<RawEvent> {
        let mut out = Vec::new();
        let mut cursor = 0;
        self.partial.extend_from_slice(bytes);
        while self.partial.len() - cursor >= 24 {
            let rec: [u8; 24] = self.partial[cursor..cursor + 24]
                .try_into()
                .expect("24 bytes");
            out.push(decode_record(&rec));
            cursor += 24;
        }
        if cursor > 0 {
            self.partial.drain(..cursor);
        }
        out
    }

    /// Bytes still waiting for a complete record.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.partial.len()
    }
}

/// One `SYN_REPORT`-terminated packet: the atomic device update.
#[derive(Clone, Debug)]
pub struct DeviceFrame {
    /// The frame's time (the `SYN_REPORT` event's timestamp).
    pub time: Mono,
    /// The events of the packet, in wire order.
    pub events: Vec<RawEvent>,
    /// The kernel reported dropped events before this frame:
    /// downstream consumers must resynchronize (multitouch tracking
    /// restarts, motion history resets).
    pub dropped: bool,
    /// A protocol-A `SYN_MT_REPORT` separator was seen: this device
    /// uses slotless multitouch, which the touch pipeline does not
    /// consume (the classifier marks it unsupported).
    pub protocol_a: bool,
}

/// Groups a decoded event stream into [`DeviceFrame`]s.
#[derive(Clone, Debug, Default)]
pub struct Framer {
    /// Events accumulated for the frame in progress.
    events: Vec<RawEvent>,
    /// Set by `SYN_DROPPED`; consumed by the next `SYN_REPORT`.
    dropped: bool,
    /// Set by `SYN_MT_REPORT`; consumed by the next `SYN_REPORT`.
    protocol_a: bool,
}

impl Framer {
    /// A framer with no frame in progress.
    #[must_use]
    pub fn new() -> Framer {
        Framer::default()
    }

    /// Feed one event; returns a completed frame when the event is
    /// `SYN_REPORT`.
    ///
    /// The `SYN_REPORT` event itself is *not* included in the frame's
    /// event list — it carries no device state, only the boundary (and
    /// the timestamp, which becomes the frame time).
    pub fn feed(&mut self, ev: RawEvent) -> Option<DeviceFrame> {
        if ev.ev_type != ev::SYN {
            self.events.push(ev);
            return None;
        }
        match ev.code {
            syn::REPORT => {
                let frame = DeviceFrame {
                    time: ev.mono(),
                    events: core::mem::take(&mut self.events),
                    dropped: core::mem::take(&mut self.dropped),
                    protocol_a: core::mem::take(&mut self.protocol_a),
                };
                Some(frame)
            }
            syn::DROPPED => {
                self.dropped = true;
                None
            }
            syn::MT_REPORT => {
                self.protocol_a = true;
                None
            }
            _ => None,
        }
    }

    /// Feed a slice of events, collecting completed frames.
    #[must_use]
    pub fn feed_all(&mut self, events: &[RawEvent]) -> Vec<DeviceFrame> {
        events.iter().filter_map(|e| self.feed(*e)).collect()
    }

    /// Events buffered for the frame in progress.
    #[must_use]
    pub fn pending(&self) -> &[RawEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: u64 = 1_000_000;

    fn ev(t_us: u64, ev_type: u16, code: u16, value: i32) -> RawEvent {
        RawEvent::new(t_us, ev_type, code, value)
    }

    #[test]
    fn record_round_trip() {
        for value in [0, 1, -1, i32::MAX, i32::MIN] {
            let e = ev(123_456_789, ev::REL, crate::codes::rel::X, value);
            let b = e.to_bytes();
            assert_eq!(decode_record(&b), e);
        }
    }

    #[test]
    fn timeval_folding() {
        // sec=2, usec=500_000 → 2_500_000 µs.
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&2i64.to_le_bytes());
        b[8..16].copy_from_slice(&500_000i64.to_le_bytes());
        b[16..18].copy_from_slice(&ev::KEY.to_le_bytes());
        b[18..20].copy_from_slice(&42u16.to_le_bytes());
        b[20..24].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(decode_record(&b).time_us, 2_500_000);
        // Negative seconds clamp to the usec field alone.
        b[0..8].copy_from_slice(&(-1i64).to_le_bytes());
        assert_eq!(decode_record(&b).time_us, 500_000);
    }

    #[test]
    fn stream_decoder_handles_partial_records() {
        let mut d = StreamDecoder::new();
        let e1 = ev(US, ev::REL, 0, 5);
        let e2 = ev(US, ev::REL, 1, -3);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&e1.to_bytes());
        bytes.extend_from_slice(&e2.to_bytes());
        // Feed in awkward chunks: 10 bytes, 24 bytes, then the rest.
        assert!(d.feed(&bytes[..10]).is_empty());
        assert_eq!(d.buffered(), 10);
        // 10 buffered + 24 fed = 34 → one record completes, 10 remain.
        assert_eq!(d.feed(&bytes[10..34]).len(), 1);
        assert_eq!(d.buffered(), 10);
        // 10 remaining + the last 14 bytes complete record two.
        let done = d.feed(&bytes[34..]);
        assert_eq!(done, vec![e2]);
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn framer_groups_until_syn_report() {
        let mut f = Framer::new();
        let motions = [
            ev(US, ev::REL, crate::codes::rel::X, 4),
            ev(US, ev::REL, crate::codes::rel::Y, 6),
        ];
        assert!(f.feed(motions[0]).is_none());
        assert!(f.feed(motions[1]).is_none());
        assert_eq!(f.pending().len(), 2);
        let frame = f.feed(ev(US + 100, ev::SYN, syn::REPORT, 0)).unwrap();
        assert_eq!(frame.events, motions);
        assert_eq!(frame.time, Mono::from_ns((US + 100) * 1000));
        assert!(!frame.dropped);
        assert!(!frame.protocol_a);
        assert!(f.pending().is_empty());
    }

    #[test]
    fn syn_dropped_marks_next_frame() {
        let mut f = Framer::new();
        assert!(f.feed(ev(US, ev::SYN, syn::DROPPED, 0)).is_none());
        assert!(f.feed(ev(US + 10, ev::KEY, 30, 1)).is_none());
        let frame = f.feed(ev(US + 20, ev::SYN, syn::REPORT, 0)).unwrap();
        assert!(frame.dropped);
        // The flag is consumed: the next frame is clean again.
        assert!(f.feed(ev(US + 30, ev::KEY, 30, 0)).is_none());
        let clean = f.feed(ev(US + 40, ev::SYN, syn::REPORT, 0)).unwrap();
        assert!(!clean.dropped);
    }

    #[test]
    fn protocol_a_separator_flagged() {
        let mut f = Framer::new();
        f.feed(ev(US, ev::ABS, crate::codes::abs::X, 100));
        f.feed(ev(US, ev::SYN, syn::MT_REPORT, 0));
        let frame = f.feed(ev(US + 5, ev::SYN, syn::REPORT, 0)).unwrap();
        assert!(frame.protocol_a);
    }

    #[test]
    fn mono_conversion() {
        assert_eq!(ev(1_500_000, ev::REL, 0, 0).mono(), Mono::from_ms(1500));
    }
}
