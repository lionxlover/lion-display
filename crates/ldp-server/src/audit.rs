//! Audit records and the sink interface.
//!
//! Every security-relevant event the server core observes is reported
//! through [`AuditSink`] (`docs/threat-model.md` §5): connections with
//! credentials, handshakes, object lifecycle, denials, fatal errors,
//! disconnects, and rejections. The sink is shared (one per server) and
//! called from session threads — implementations must be `Send + Sync`
//! and non-blocking on the session path.
//!
//! Phase 4 ships the interface points; the `ldp.security` audit *broker*
//! (streaming records to clients, tamper-evident storage) is a later
//! phase. Two implementations are provided: [`NullAudit`] (default, zero
//! cost) and [`AuditRecorder`] (bounded in-memory ring for tests and
//! diagnostics).

use std::sync::Mutex;

use ldp_core::error::ErrorCode;
use ldp_core::ids::{ClientId, ObjectId};

use crate::session::SessionEnd;

/// One audit-worthy event. Records are cheap to clone and carry no
/// secrets (object IDs, interface names, counters — never credentials
/// beyond the SO_PEERCRED identity triple, which is the point).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditRecord {
    /// A connection was accepted and its credentials read.
    Connected {
        /// Server-assigned identity for the connection.
        client: ClientId,
        /// Peer pid from `SO_PEERCRED`.
        pid: i32,
        /// Peer uid.
        uid: u32,
        /// Peer gid.
        gid: u32,
    },
    /// The handshake completed; `options` is the *granted* set.
    Handshake {
        /// Which connection.
        client: ClientId,
        /// Client's claimed LDP release number.
        release: u32,
        /// Options the server granted (a subset of what was asked).
        options: ldp_core::bitset::Bitset128,
    },
    /// An object was created (registry bind or get_registry).
    Bind {
        /// Which connection.
        client: ClientId,
        /// Fully qualified interface name.
        interface: std::sync::Arc<str>,
        /// Pinned wire version.
        version: u32,
        /// The created object.
        object: ObjectId,
    },
    /// An object was destroyed by client request.
    Destroy {
        /// Which connection.
        client: ClientId,
        /// Interface the object had.
        interface: std::sync::Arc<str>,
        /// The destroyed object.
        object: ObjectId,
    },
    /// The server tore down an object the client did not destroy.
    Revoke {
        /// Which connection.
        client: ClientId,
        /// Interface the object had.
        interface: std::sync::Arc<str>,
        /// The revoked object.
        object: ObjectId,
        /// `revoked_reason` wire value.
        reason: u32,
    },
    /// A capability check failed (always audited; the wire message never
    /// leaks why).
    Denied {
        /// Which connection.
        client: ClientId,
        /// The denied request, for the audit trail only.
        request: &'static str,
    },
    /// A fatal protocol error ended a connection.
    Fatal {
        /// Which connection.
        client: ClientId,
        /// Stable wire code.
        code: ErrorCode,
        /// Offending object, when one was implicated.
        object: Option<ObjectId>,
        /// Diagnostic detail.
        detail: std::sync::Arc<str>,
    },
    /// A request was routed to an interface with no implementation
    /// attached (the dispatcher consumed it without acting).
    Unhandled {
        /// Which connection.
        client: ClientId,
        /// Interface of the target object.
        interface: std::sync::Arc<str>,
        /// Request opcode.
        opcode: u32,
    },
    /// The connection ended; all its objects were reclaimed.
    Disconnected {
        /// Which connection.
        client: ClientId,
        /// How it ended.
        reason: SessionEnd,
    },
    /// A connection was refused before any protocol byte was read.
    Rejected {
        /// Peer pid from `SO_PEERCRED`.
        pid: i32,
        /// Peer uid.
        uid: u32,
        /// Why (machine-stable label).
        reason: &'static str,
    },
}

/// Receiver of audit records. Implementations must be non-blocking (or
/// bound their own buffering): session threads call this inline.
pub trait AuditSink: Send + Sync {
    /// Handle one record.
    fn record(&self, record: AuditRecord);
}

/// The default sink: drops every record. Servers that want auditing
/// install a real sink in [`crate::config::ServerConfig`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NullAudit;

impl AuditSink for NullAudit {
    fn record(&self, _record: AuditRecord) {}
}

/// A bounded in-memory ring of records for tests and diagnostics.
///
/// The bound keeps a misbehaving peer from growing server memory through
/// audit traffic: once full, the oldest records are dropped (a count of
/// the dropped ones is kept).
#[derive(Debug)]
pub struct AuditRecorder {
    inner: Mutex<Vec<AuditRecord>>,
    dropped: std::sync::atomic::AtomicU64,
    capacity: usize,
}

impl AuditRecorder {
    /// A recorder holding at most `capacity` records.
    #[must_use]
    pub fn new(capacity: usize) -> AuditRecorder {
        AuditRecorder {
            inner: Mutex::new(Vec::new()),
            dropped: std::sync::atomic::AtomicU64::new(0),
            capacity: capacity.max(1),
        }
    }

    /// Snapshot the retained records (oldest first).
    ///
    /// # Panics
    ///
    /// If another thread poisoned the recorder's mutex while panicking
    /// — the recorder is only used from test and diagnostic code.
    pub fn snapshot(&self) -> Vec<AuditRecord> {
        self.inner.lock().expect("audit recorder poisoned").clone()
    }

    /// Records still retained.
    ///
    /// # Panics
    ///
    /// If another thread poisoned the recorder's mutex while panicking
    /// — the recorder is only used from test and diagnostic code.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("audit recorder poisoned").len()
    }

    /// Whether no records are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AuditSink for AuditRecorder {
    fn record(&self, record: AuditRecord) {
        let mut v = self.inner.lock().expect("audit recorder poisoned");
        if v.len() >= self.capacity {
            v.remove(0);
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        v.push(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::ids::{ClientId, ObjectId};

    #[test]
    fn recorder_keeps_newest_within_capacity() {
        let sink = AuditRecorder::new(3);
        // Object IDs start at 2 (1 is the reserved bootstrap).
        for i in 2..=6u32 {
            sink.record(AuditRecord::Bind {
                client: ClientId::new(i).unwrap(),
                interface: "ldp.core.output".into(),
                version: 1,
                object: ObjectId::client(i).unwrap(),
            });
        }
        let kept = sink.snapshot();
        assert_eq!(kept.len(), 3);
        assert_eq!(sink.dropped(), 2);
        // Oldest dropped: the newest three remain, in order.
        assert_eq!(
            kept[0],
            AuditRecord::Bind {
                client: ClientId::new(4).unwrap(),
                interface: "ldp.core.output".into(),
                version: 1,
                object: ObjectId::client(4).unwrap(),
            }
        );
    }

    #[test]
    fn null_audit_is_a_noop() {
        NullAudit.record(AuditRecord::Rejected {
            pid: 1,
            uid: 2,
            reason: "max_clients",
        });
    }
}
