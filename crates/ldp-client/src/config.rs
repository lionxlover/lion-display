//! Client configuration and reconnect policy.
//!
//! [`ClientConfig`] carries everything a connection needs before the
//! first byte: the release number advertised in `hello`, the
//! `connection_options` bits requested (the server may silently drop
//! any of them — `docs/protocol.md` §5), and the framing limits the
//! client starts under.
//!
//! The `large_messages` policy follows the spec's "granted set is
//! observable through what works" rule: a client that requests the
//! option sizes its framing for 64 MiB in *both* directions, because
//! (a) a granting server may legally send large messages, so the
//! reader must accept them, and (b) if the server dropped the option,
//! an oversized send fails with a `limit_exceeded` error — the
//! observable grant signal.

use ldp_core::bitset::Bitset128;
use ldp_core::limits::Limits;
use std::time::Duration;

/// Bit 0 of `connection_options`: enable `registry.introspect`.
pub const OPT_INTROSPECTION: u32 = 0;
/// Bit 1 of `connection_options`: raise the message ceiling to 64 MiB.
pub const OPT_LARGE_MESSAGES: u32 = 1;

/// The release number this library speaks (`spec/README.md` versioning;
/// protocol v1 releases start at 1).
pub const CLIENT_RELEASE: u32 = 1;

/// Everything a connection needs before the handshake.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Release number sent in `connection.hello`. Diagnostics only; the
    /// server never gates on it (v1 has exactly release 1).
    pub release: u32,
    /// `connection_options` bits requested in `hello`. Requests are
    /// promises the client can keep: `large_messages` widens this
    /// connection's framing to 64 MiB immediately after the request is
    /// sent.
    pub options: Bitset128,
    /// Framing limits before negotiation. The `large_messages` request
    /// switches the connection to [`Limits::LARGE_MESSAGES`].
    pub limits: Limits,
    /// Handshake timeout: how long to wait for `welcome` before giving
    /// up (a server that never answers is not worth blocking on).
    pub handshake_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            release: CLIENT_RELEASE,
            options: Bitset128::EMPTY,
            limits: Limits::DEFAULT,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

impl ClientConfig {
    /// A configuration requesting `connection_options.introspection`.
    #[must_use]
    pub fn with_introspection(mut self) -> ClientConfig {
        self.options = self.options.with(OPT_INTROSPECTION);
        self
    }

    /// A configuration requesting `connection_options.large_messages`.
    #[must_use]
    pub fn with_large_messages(mut self) -> ClientConfig {
        self.options = self.options.with(OPT_LARGE_MESSAGES);
        self
    }

    /// Whether `large_messages` is requested (drives the post-hello
    /// framing switch).
    #[must_use]
    pub fn wants_large_messages(&self) -> bool {
        self.options.test(OPT_LARGE_MESSAGES)
    }
}

/// Reconnect schedule: bounded exponential backoff with seeded jitter.
///
/// Jitter avoids thundering-herd reconnects and comes from a
/// deterministic per-connection LCG (no `rand` dependency, reproducible
/// in tests). The policy caps total attempts; `0` attempts means "try
/// forever" is *not* expressible on purpose — a client that cannot
/// reconnect should surface the failure to its user, not spin silently.
#[derive(Clone, Debug)]
pub struct ReconnectPolicy {
    /// Maximum connection attempts (the first try counts).
    pub max_attempts: u32,
    /// Delay before the second attempt.
    pub initial_delay: Duration,
    /// Backoff ceiling per delay.
    pub max_delay: Duration,
    /// Delay multiplier between attempts (backoff growth). Must be >= 1.
    pub factor: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        ReconnectPolicy {
            max_attempts: 8,
            initial_delay: Duration::from_millis(25),
            max_delay: Duration::from_secs(2),
            factor: 2,
        }
    }
}

impl ReconnectPolicy {
    /// A fast policy for tests and local sockets.
    #[must_use]
    pub fn fast() -> ReconnectPolicy {
        ReconnectPolicy {
            max_attempts: 20,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            factor: 2,
        }
    }

    /// The delay before attempt `attempt` (1-based: 1 = the first
    /// retry). Exponential growth from [`Self::initial_delay`] by
    /// [`Self::factor`], capped at [`Self::max_delay`], plus jitter of
    /// up to half the nominal delay.
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        if self.factor < 1 || attempt == 0 {
            return Duration::ZERO;
        }
        // saturating_ms: nominal = initial * factor^(attempt-1)
        let mut ms = self.initial_delay.as_millis() as u64;
        let mut growth = attempt - 1;
        while growth > 0 {
            ms = ms.saturating_mul(u64::from(self.factor));
            if ms >= self.max_delay.as_millis() as u64 {
                ms = self.max_delay.as_millis() as u64;
                break;
            }
            growth -= 1;
        }
        let cap = self.max_delay.as_millis() as u64;
        if ms > cap {
            ms = cap;
        }
        // Deterministic jitter: an xorshift over the attempt number,
        // bounded to half the nominal delay. `attempt` mixes the seed
        // so consecutive retries do not jitter identically.
        let jitter = Self::jitter(attempt) % (ms / 2 + 1);
        Duration::from_millis(ms + jitter)
    }

    /// A tiny deterministic PRNG step (xorshift32).
    fn jitter(seed: u32) -> u64 {
        let mut x = seed.wrapping_mul(2_655_443_576).wrapping_add(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        u64::from(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_requests_nothing() {
        let c = ClientConfig::default();
        assert!(c.options.is_empty());
        assert_eq!(c.release, CLIENT_RELEASE);
        assert_eq!(c.limits, Limits::DEFAULT);
        assert!(!c.wants_large_messages());
    }

    #[test]
    fn option_builders_set_bits() {
        let c = ClientConfig::default()
            .with_introspection()
            .with_large_messages();
        assert!(c.options.test(OPT_INTROSPECTION));
        assert!(c.options.test(OPT_LARGE_MESSAGES));
        assert!(c.wants_large_messages());
    }

    #[test]
    fn backoff_grows_then_caps() {
        let p = ReconnectPolicy {
            max_attempts: 12,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(80),
            factor: 2,
        };
        let d1 = p.delay_for(1).as_millis();
        // 10 nominal + up to 5 jitter
        assert!((10..=15).contains(&d1), "first retry near nominal: {d1}");
        let d6 = p.delay_for(6).as_millis();
        // nominal caps at 80; jitter adds up to 40
        assert!((80..=120).contains(&d6), "growth capped: {d6}");
        // monotone up to the cap (jitter can wobble below the previous
        // nominal only after capping)
        let d3 = p.delay_for(3).as_millis();
        assert!(d3 >= 30, "attempt 3 at least nominal 30: {d3}");
    }

    #[test]
    fn zeroth_attempt_has_no_delay() {
        let p = ReconnectPolicy::default();
        assert_eq!(p.delay_for(0), Duration::ZERO);
    }

    #[test]
    fn jitter_is_deterministic() {
        assert_eq!(ReconnectPolicy::jitter(7), ReconnectPolicy::jitter(7));
        assert_ne!(ReconnectPolicy::jitter(7), ReconnectPolicy::jitter(8));
    }
}
