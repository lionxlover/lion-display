//! Disconnect and reconnect helpers.
//!
//! An LDP connection is stateful end to end: every proxy, every
//! negotiated version, every registry binding dies with the socket.
//! Reconnecting is therefore not "retry the send" but "rebuild the
//! session" — and the only component that knows what the session
//! *was* is the application. [`Reconnector`] packages the loop:
//!
//! 1. connect + handshake under a fixed [`ClientConfig`],
//! 2. hand the fresh [`Connection`] to an application rebuilder that
//!    re-creates its objects (`get_registry`, `bind`, factory calls),
//! 3. drive it until the application reports the session is whole,
//! 4. on failure: sleep per the [`ReconnectPolicy`] schedule and retry,
//!    until attempts run out.
//!
//! The rebuild callback runs between "socket works" and "session
//! works", which is exactly where application state restoration
//! belongs (re-read outputs, re-create surfaces, repaint).
//!
//! ```
//! use std::time::Duration;
//! use ldp_client::{ClientConfig, ReconnectPolicy};
//! use ldp_transport::UnixAddr;
//!
//! # fn main() -> ldp_client::Result<()> {
//! let addr = UnixAddr::abstract_name(b"\0ldp-example").unwrap();
//! let policy = ReconnectPolicy {
//!     max_attempts: 3,
//!     initial_delay: Duration::from_millis(5),
//!     max_delay: Duration::from_millis(20),
//!     factor: 2,
//! };
//! let reconnector = ldp_client::Reconnector::new(ClientConfig::default(), addr.clone(), policy);
//! # let _ = reconnector;
//! // `run` would connect and call the rebuilder until the session is
//! // whole or attempts are exhausted; see the integration tests.
//! # Ok(())
//! # }
//! ```

use std::thread::sleep;

use ldp_transport::addr::UnixAddr;

use crate::config::{ClientConfig, ReconnectPolicy};
use crate::connection::Connection;
use crate::error::{ClientError, Result};

/// Session rebuilder: re-create application state on a fresh connection.
///
/// Return `Ok(())` once the session is whole (all objects re-bound and
/// confirmed — ending with a `roundtrip` is the natural pattern);
/// return `Err` to trigger the next retry.
pub trait Rebuild {
    /// Rebuild the application session on `conn`.
    ///
    /// # Errors
    ///
    /// Any failure that means "this attempt did not produce a whole
    /// session" — the reconnector schedules the next attempt.
    fn rebuild(&mut self, conn: &mut Connection) -> Result<()>;
}

/// Blanket impl for closures.
impl<F> Rebuild for F
where
    F: FnMut(&mut Connection) -> Result<()>,
{
    fn rebuild(&mut self, conn: &mut Connection) -> Result<()> {
        self(conn)
    }
}

/// Why a [`Reconnector::run`] loop ended.
#[derive(Clone, Debug)]
pub enum ReconnectOutcome {
    /// The session is whole; the connection is live.
    Connected,
    /// Attempts ran out; the last error is carried for the UI.
    Exhausted {
        /// Attempts made (including the first).
        attempts: u32,
        /// The failure that ended the last attempt.
        last: ClientError,
    },
}

/// The reconnect driver: fixed config, fixed address, fixed schedule.
pub struct Reconnector {
    config: ClientConfig,
    addr: UnixAddr,
    policy: ReconnectPolicy,
    /// Attempts made in the current loop (0 between loops).
    attempt_counter: u32,
}

impl Reconnector {
    /// Build a reconnector (immutable; reusable across outages).
    #[must_use]
    pub fn new(config: ClientConfig, addr: UnixAddr, policy: ReconnectPolicy) -> Reconnector {
        Reconnector {
            config,
            addr,
            policy,
            attempt_counter: 0,
        }
    }

    /// The schedule this reconnector follows.
    #[must_use]
    pub fn policy(&self) -> &ReconnectPolicy {
        &self.policy
    }

    /// One attempt: connect, handshake, rebuild.
    fn attempt(&mut self, rebuild: &mut dyn Rebuild) -> Result<Connection> {
        self.attempt_counter += 1;
        let mut conn = Connection::connect_with(self.config.clone(), &self.addr)?;
        rebuild.rebuild(&mut conn)?;
        Ok(conn)
    }

    /// Run the loop: first attempt immediately, then scheduled retries.
    /// Returns the live connection with its rebuilt session, or the
    /// exhaustion report.
    ///
    /// # Errors
    ///
    /// [`ReconnectOutcome::Exhausted`] (as the `Err` side) once
    /// [`ReconnectPolicy::max_attempts`] attempts failed; the variant
    /// carries the last attempt's error for reporting.
    pub fn run(&mut self, rebuild: &mut dyn Rebuild) -> Result<Connection, ReconnectOutcome> {
        self.attempt_counter = 0; // fresh schedule per loop
        let deadline_attempts = self.policy.max_attempts.max(1);
        loop {
            match self.attempt(rebuild) {
                Ok(conn) => return Ok(conn),
                Err(e) => {
                    let attempts_done = self.attempts_done();
                    if attempts_done >= deadline_attempts {
                        return Err(ReconnectOutcome::Exhausted {
                            attempts: attempts_done,
                            last: e,
                        });
                    }
                }
            }
            // Scheduled wait before the next attempt (attempt N waits
            // delay_for(N): the first retry waits initial_delay).
            let delay = self.policy.delay_for(self.attempts_done());
            // A cap keeps a misconfigured policy (huge delays) from
            // parking forever: never sleep past 60s per step.
            let capped = delay.min(std::time::Duration::from_secs(60));
            if !capped.is_zero() {
                sleep(capped);
            }
        }
    }

    /// Attempts made so far in the current `run` loop (1-based; 0
    /// before the loop and after it returns).
    #[must_use]
    pub fn attempts_done(&self) -> u32 {
        self.attempt_counter
    }
}

/// A rebuild step that does nothing (the session is just the
/// handshake). Used by [`connect_with_retry`].
struct NoopRebuild;

impl Rebuild for NoopRebuild {
    fn rebuild(&mut self, _conn: &mut Connection) -> Result<()> {
        Ok(())
    }
}

/// One-shot helper: connect with retries, no application rebuild.
///
/// # Errors
///
/// [`ClientError::Ldp`] when every attempt failed (transport-level);
/// the outcome carries no `ReconnectOutcome` because there is no
/// rebuild step to fail.
pub fn connect_with_retry(
    config: ClientConfig,
    addr: &UnixAddr,
    policy: ReconnectPolicy,
) -> Result<Connection> {
    let mut reconnector = Reconnector::new(config, addr.clone(), policy);
    reconnector
        .run(&mut NoopRebuild)
        .map_err(|outcome| match outcome {
            ReconnectOutcome::Exhausted { last, .. } => last,
            ReconnectOutcome::Connected => unreachable!("run returns Ok on Connected"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn closures_implement_rebuild() {
        // Type-level check only: the blanket closure impl coerces to
        // the trait object (lifetime-generic signature).
        fn takes_dyn(_r: &mut dyn Rebuild) {}
        let mut f = |_: &mut Connection| Ok(());
        takes_dyn(&mut f);
    }

    #[test]
    fn outcome_display_shapes() {
        let o = ReconnectOutcome::Exhausted {
            attempts: 3,
            last: ClientError::ConnectionDead,
        };
        assert!(format!("{o:?}").contains("Exhausted"));
    }

    #[test]
    fn reconnector_exposes_policy() {
        let r = Reconnector::new(
            ClientConfig::default(),
            UnixAddr::abstract_name(b"\0x").unwrap(),
            ReconnectPolicy::fast(),
        );
        assert_eq!(r.policy().max_attempts, 20);
    }

    /// The loop counts attempts and gives up at the cap: a counting
    /// rebuild that always fails must be called exactly max_attempts
    /// times (no sleeps thanks to the fast policy).
    #[test]
    fn attempts_are_bounded() {
        let addr = UnixAddr::abstract_name(
            format!("\u{0}ldp-reconn-test-{}", std::process::id()).as_bytes(),
        )
        .unwrap();
        let policy = ReconnectPolicy {
            max_attempts: 4,
            initial_delay: std::time::Duration::ZERO,
            max_delay: std::time::Duration::ZERO,
            factor: 1,
        };
        let mut reconnector = Reconnector::new(ClientConfig::default(), addr, policy);
        let counter = Arc::new(AtomicU32::new(0));
        let mut rebuild = CounterRebuild {
            count: counter.clone(),
        };
        let outcome = reconnector.run(&mut rebuild);
        assert!(outcome.is_err());
        match outcome.unwrap_err() {
            ReconnectOutcome::Exhausted { attempts, last } => {
                assert_eq!(attempts, 4);
                assert!(matches!(last, ClientError::Ldp(_)));
            }
            ReconnectOutcome::Connected => panic!("cannot connect to nothing"),
        }
        // The rebuild never ran: every attempt died at connect().
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// A rebuild step that counts invocations (never reached when the
    /// server is absent — every attempt dies at connect).
    struct CounterRebuild {
        count: Arc<AtomicU32>,
    }

    impl Rebuild for CounterRebuild {
        fn rebuild(&mut self, _conn: &mut Connection) -> Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Err(ClientError::ConnectionDead)
        }
    }
}
