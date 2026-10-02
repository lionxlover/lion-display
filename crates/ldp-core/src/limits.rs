//! Protocol limits.
//!
//! The defaults of `docs/protocol.md` §10 as one place of truth. Every
//! ceiling is enforced *before* allocation proportional to untrusted sizes
//! (validation pipeline order is a security property —
//! `docs/protocol.md` §9). Servers may raise the message ceiling via the
//! `large_messages` connection option; the rest are structural.

/// The protocol limit set in force on one connection.
///
/// Copy, cheap, and derivable from the negotiated `hello`/`welcome`
/// options. Comparisons are explicit so a future negotiation change can
/// widen a limit without touching call sites.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Maximum total message size (header + payload).
    pub message_bytes: u64,
    /// Maximum FDs carried by one message.
    pub fds_per_message: u32,
    /// Maximum open FDs held by one client.
    pub client_fds: u32,
    /// Maximum string argument length (bytes, excluding NUL).
    pub string_bytes: u32,
    /// Maximum live objects per client.
    pub client_objects: u32,
    /// Maximum rects per region/damage argument.
    pub region_rects: u32,
    /// Maximum queued outbound event bytes per client.
    pub event_queue_bytes: u64,
    /// Maximum buffer bytes mapped per client (policy knob).
    pub client_buffer_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

impl Limits {
    /// The v1 defaults (`docs/protocol.md` §10).
    pub const DEFAULT: Limits = Limits {
        message_bytes: 1024 * 1024,
        fds_per_message: 64,
        client_fds: 256,
        string_bytes: 4096,
        client_objects: 1 << 20,
        region_rects: 4096,
        event_queue_bytes: 2 * 1024 * 1024,
        client_buffer_bytes: 512 * 1024 * 1024,
    };

    /// The `large_messages` negotiation: 64 MiB message ceiling. FD and
    /// structural limits are unchanged — only payload size widens.
    pub const LARGE_MESSAGES: Limits = Limits {
        message_bytes: 64 * 1024 * 1024,
        ..Limits::DEFAULT
    };

    /// Message-header size in bytes (`docs/protocol.md` §2).
    pub const HEADER_BYTES: u64 = 16;

    /// Wire alignment unit.
    pub const ALIGN: u64 = 8;

    /// Maximum payload words for a message under `self` (header excluded).
    #[must_use]
    pub const fn max_payload_words(self) -> u64 {
        (self.message_bytes - Self::HEADER_BYTES) / Self::ALIGN
    }

    /// Validate a framed message length (payload words + header) against
    /// the message ceiling.
    #[must_use]
    pub const fn message_fits(self, payload_words: u64) -> bool {
        payload_words <= self.max_payload_words()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_the_documented_table() {
        let l = Limits::DEFAULT;
        assert_eq!(l.message_bytes, 1024 * 1024);
        assert_eq!(l.fds_per_message, 64);
        assert_eq!(l.client_fds, 256);
        assert_eq!(l.string_bytes, 4096);
        assert_eq!(l.client_objects, 1 << 20);
        assert_eq!(l.region_rects, 4096);
        assert_eq!(l.event_queue_bytes, 2 * 1024 * 1024);
        assert_eq!(l.client_buffer_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn large_messages_only_widens_payload() {
        let l = Limits::LARGE_MESSAGES;
        assert_eq!(l.message_bytes, 64 * 1024 * 1024);
        assert_eq!(l.fds_per_message, Limits::DEFAULT.fds_per_message);
        assert_eq!(l.string_bytes, Limits::DEFAULT.string_bytes);
    }

    #[test]
    fn message_fits_math() {
        let l = Limits::DEFAULT;
        assert!(l.message_fits(0));
        assert!(l.message_fits((l.message_bytes - Limits::HEADER_BYTES) / Limits::ALIGN));
        // One word over the ceiling fails.
        assert!(!l.message_fits(l.max_payload_words() + 1));
        // The full 1 MiB minus header is exactly at the boundary.
        let words = (1024 * 1024 - 16) / 8;
        assert_eq!(words, 131_070);
        assert!(l.message_fits(words));
    }

    #[test]
    fn header_and_alignment_constants() {
        assert_eq!(Limits::HEADER_BYTES, 16);
        assert_eq!(Limits::ALIGN, 8);
        assert_eq!(Limits::HEADER_BYTES % Limits::ALIGN, 0);
    }
}
