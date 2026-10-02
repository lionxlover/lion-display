//! The server: acceptance, session threads, resource ceilings.
//!
//! One [`Server`] owns the configuration, the client-ID allocator, and
//! the live-session ceiling. [`Server::accept_session`] performs the
//! accept-with-credentials step and either returns a ready session or
//! `None` (connection refused at the ceiling — closed and audited before
//! any protocol byte is processed).
//!
//! [`Server::serve_blocking`] is the phase-4 drive model: a blocking
//! accept loop that spawns one thread per connection, each running
//! [`ClientSession::run`] to completion. The threads are detached and
//! counted: `live_sessions()` is the ceiling's measure. Panicking
//! sessions are caught (unwind builds) and audited; under
//! `panic = "abort"` release policy the process dies loudly instead,
//! which is the intended loud failure for a display server bug.
//!
//! Embedders that want their own event loop (the Phase 6 compositor)
//! use `accept_session` + `ClientSession::step` directly and call
//! [`Server::session_finished`] when a session ends by dropping.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::ClientId;
use ldp_transport::error::is_would_block;
use ldp_transport::listener::TransportListener;

use crate::audit::AuditRecord;
use crate::config::ServerConfig;
use crate::dispatch::Dispatcher;
use crate::session::ClientSession;

/// The acceptance and lifetime state shared across session threads.
#[derive(Debug, Default)]
struct Shared {
    next_client: AtomicU32,
    live: AtomicU32,
}

/// A configured LDP server core.
pub struct Server {
    config: Arc<ServerConfig>,
    shared: Arc<Shared>,
}

/// Manual `Debug`: the configuration holds a trait object.
impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("max_clients", &self.config.max_clients)
            .field("globals", &self.config.globals.len())
            .field("live_sessions", &self.live_sessions())
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Build a server over a validated configuration.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] when the configuration is inconsistent (an
    /// advertised global not in the compiled schema, a zero client
    /// ceiling, or a zero release number) — a deployment bug, not peer
    /// input.
    pub fn new(config: ServerConfig) -> Result<Server> {
        config.validate()?;
        Ok(Server {
            config: Arc::new(config),
            shared: Arc::new(Shared::default()),
        })
    }

    /// The configuration this server runs under.
    #[must_use]
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Sessions currently reserved (accepted, not yet finished). The
    /// `max_clients` ceiling counts exactly this.
    #[must_use]
    pub fn live_sessions(&self) -> u32 {
        self.shared.live.load(Ordering::Acquire)
    }

    /// Accept one connection. Blocking: parks until a peer connects
    /// (use a nonblocking listener and match
    /// [`is_would_block`] to poll).
    ///
    /// `Ok(None)` means the connection was refused at the client
    /// ceiling: it is already closed and audited — no protocol byte of
    /// it was read. `Ok(Some(session))` reserves a live-session slot;
    /// drive the session to completion (run it, then
    /// [`Server::session_finished`]) or embedders leak the count.
    ///
    /// # Errors
    ///
    /// Transport failures from `accept4`/`SO_PEERCRED`, or session
    /// construction failures (both close the accepted socket).
    pub fn accept_session(
        &self,
        listener: &mut TransportListener,
    ) -> Result<Option<ClientSession>> {
        let (stream, creds) = listener.accept()?;
        let reserved = self.shared.live.fetch_add(1, Ordering::AcqRel) + 1;
        if reserved > self.config.max_clients {
            self.shared.live.fetch_sub(1, Ordering::AcqRel);
            self.config.audit.record(AuditRecord::Rejected {
                pid: creds.pid,
                uid: creds.uid,
                reason: "max_clients",
            });
            // Dropping the stream closes it: refusal before any read.
            return Ok(None);
        }
        match self.next_client_id() {
            Ok(id) => {
                self.config.audit.record(AuditRecord::Connected {
                    client: id,
                    pid: creds.pid,
                    uid: creds.uid,
                    gid: creds.gid,
                });
                match ClientSession::new(stream, creds, id, Arc::clone(&self.config)) {
                    Ok(session) => Ok(Some(session)),
                    Err(e) => {
                        self.session_finished();
                        Err(e)
                    }
                }
            }
            Err(e) => {
                self.session_finished();
                Err(e)
            }
        }
    }

    fn next_client_id(&self) -> Result<ClientId> {
        let mut n = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        if n == 0 {
            // Wrapped past u32::MAX once: skip the reserved zero.
            n = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        }
        ClientId::new(n).ok_or(LdpError::Logic {
            what: "client-id allocator exhausted",
        })
    }

    /// Release a session's live-slot reservation. Called automatically
    /// by [`Server::spawn_session`]'s thread when a session ends;
    /// embedders driving sessions manually call this after the session
    /// object is dropped.
    pub fn session_finished(&self) {
        self.shared.live.fetch_sub(1, Ordering::AcqRel);
    }

    /// Run one session to completion on a new thread (detached).
    ///
    /// The dispatcher is created by `make_dispatcher` on the session
    /// thread (per-session state; wrap shared state in `Arc<Mutex<…>>`).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`] when the thread cannot be spawned (resource
    /// exhaustion — the caller decides whether to keep serving).
    pub fn spawn_session<D>(
        &self,
        session: ClientSession,
        make_dispatcher: impl FnOnce() -> D + Send + 'static,
    ) -> Result<()>
    where
        D: Dispatcher + 'static,
    {
        let config = Arc::clone(&self.config);
        let shared = Arc::clone(&self.shared);
        let client = session.client_id();
        std::thread::Builder::new()
            .name("ldp-session".into())
            .spawn(move || {
                let mut session = session;
                let mut dispatcher = make_dispatcher();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    session.run(&mut dispatcher)
                }));
                drop(dispatcher);
                drop(session);
                // Release the live slot only after the session (and its
                // socket) is gone: `live == 0` must imply every FD of the
                // session is closed, or resource-flatness checks race.
                if outcome.is_err() {
                    config.audit.record(AuditRecord::Fatal {
                        client,
                        code: ErrorCode::ServerError,
                        object: None,
                        detail: "session thread panicked".into(),
                    });
                }
                shared.live.fetch_sub(1, Ordering::AcqRel);
            })
            .map(|_| ())
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))
    }

    /// The blocking serve loop: accept, spawn, repeat. Requires a
    /// blocking listener; returns only on listener failure.
    ///
    /// # Errors
    ///
    /// Propagates accept failures (including `WouldBlock` on a
    /// nonblocking listener — that is a misuse of this entry point, use
    /// `accept_session` directly for polling loops).
    pub fn serve_blocking<F, D>(
        &self,
        listener: &mut TransportListener,
        make_dispatcher: F,
    ) -> Result<()>
    where
        F: Fn() -> D + Send + Sync + Clone + 'static,
        D: Dispatcher + 'static,
    {
        loop {
            match self.accept_session(listener) {
                Ok(Some(session)) => {
                    self.spawn_session(session, make_dispatcher.clone())?;
                }
                Ok(None) => {}
                Err(e) => {
                    if is_would_block(&e) {
                        return Err(LdpError::Logic {
                            what: "serve_blocking requires a blocking listener",
                        });
                    }
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditRecorder;
    use crate::config::GlobalAdvert;
    use ldp_transport::addr::UnixAddr;

    fn addr(tag: &str) -> UnixAddr {
        UnixAddr::abstract_name(format!("ldp-server-unit-{tag}-{}", std::process::id()).as_bytes())
            .unwrap()
    }

    #[test]
    fn rejects_unknown_globals_at_construction() {
        let cfg = ServerConfig {
            globals: vec![GlobalAdvert::new("ldp.nope")],
            ..ServerConfig::default()
        };
        assert!(Server::new(cfg).is_err());
    }

    #[test]
    fn ceiling_refuses_and_releases() {
        let audit = Arc::new(AuditRecorder::new(64));
        let cfg = ServerConfig {
            max_clients: 1,
            audit: Arc::clone(&audit) as Arc<dyn crate::audit::AuditSink>,
            ..ServerConfig::default()
        };
        let server = Server::new(cfg).unwrap();
        let a = addr("ceiling");
        let mut listener = TransportListener::bind(&a, 4).unwrap();

        // First client reserves the only slot and parks on the socket.
        let _parked = ldp_transport::stream::TransportStream::connect(&a).unwrap();
        let session = server.accept_session(&mut listener).unwrap().unwrap();
        assert_eq!(server.live_sessions(), 1);

        // Second client is refused before any byte is read: the server
        // closed its end, so the client sees EOF immediately.
        let mut refused = ldp_transport::stream::TransportStream::connect(&a).unwrap();
        assert!(server.accept_session(&mut listener).unwrap().is_none());
        let mut probe = [0u8; 1];
        let got = refused.recv_chunk_plain(&mut probe).unwrap();
        assert_eq!(
            got.bytes, 0,
            "refused connection must be closed by the server"
        );
        drop(refused);

        // Dropping the session does not release the slot by itself…
        drop(session);
        assert_eq!(server.live_sessions(), 1);
        // …the embedder releases explicitly.
        server.session_finished();
        assert_eq!(server.live_sessions(), 0);

        let records = audit.snapshot();
        assert!(records
            .iter()
            .any(|r| matches!(r, AuditRecord::Connected { .. })));
        assert!(records.iter().any(|r| matches!(
            r,
            AuditRecord::Rejected {
                reason: "max_clients",
                ..
            }
        )));
    }

    #[test]
    fn null_audit_default_config_builds() {
        let cfg = ServerConfig::default();
        assert!(Server::new(cfg).is_ok());
    }
}
