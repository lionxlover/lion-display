//! The D-Bus connection state machine over a transport seam.
//!
//! [`DbusTransport`] is the byte pipe: a real deployment binds a
//! unix-socket peer (auth handshake + SCM_RIGHTS); the hermetic tests
//! script it. The connection owns serial allocation and reply
//! matching; [`DbusConnection::on_message`] turns parsed messages into
//! typed events.

use std::collections::BTreeMap;

use crate::dbus::{parse_message, DbusError, DbusMessage, DbusValue, MessageType};

/// The byte/fd pipe to a D-Bus daemon. `fds` are transport-level
/// tokens (raw fds in production, arbitrary handles in tests).
pub trait DbusTransport {
    /// Send one marshaled message with its fd list.
    ///
    /// # Errors
    ///
    /// [`DbusError`] when the pipe is broken or rejects the write.
    fn send(&mut self, bytes: &[u8], fds: &[u32]) -> Result<(), DbusError>;
    /// Receive the next complete message, if one is ready.
    ///
    /// # Errors
    ///
    /// [`DbusError`] when the pipe is broken or the bytes are malformed.
    fn recv(&mut self) -> Result<Option<DbusMessage>, DbusError>;
}

/// One outgoing call, marshaled and ready for the transport.
#[derive(Clone, Debug)]
pub struct OutgoingCall {
    /// The serial to match the reply on.
    pub serial: u32,
    /// Marshaled bytes.
    pub bytes: Vec<u8>,
    /// fd tokens riding along (usually empty on calls).
    pub fds: Vec<u32>,
    /// What the call was (reply matching + diagnostics).
    pub member: &'static str,
}

/// Events surfaced to the session layer.
#[derive(Clone, Debug, PartialEq)]
pub enum DbusEvent {
    /// A successful method return.
    Reply {
        /// Serial of the reply message.
        serial: u32,
        /// The serial it answers.
        reply_to: u32,
        /// The member that call was for.
        member: &'static str,
        /// Reply body values.
        values: Vec<DbusValue>,
    },
    /// An error return.
    Error {
        /// The serial it answers.
        reply_to: u32,
        /// The member that call was for.
        member: &'static str,
        /// D-Bus error name (e.g. `org.freedesktop.DBus.Error.AccessDenied`).
        name: String,
        /// Error body values.
        values: Vec<DbusValue>,
    },
    /// A broadcast signal.
    Signal {
        /// Sender interface.
        interface: String,
        /// Member name.
        member: String,
        /// Object path.
        path: String,
        /// Signal body values.
        values: Vec<DbusValue>,
    },
}

/// Serial allocation + reply matching.
#[derive(Debug)]
pub struct DbusConnection {
    next_serial: u32,
    pending: BTreeMap<u32, &'static str>,
}

impl Default for DbusConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl DbusConnection {
    /// A fresh connection (serials start at 1; 0 is reserved).
    #[must_use]
    pub const fn new() -> Self {
        DbusConnection {
            next_serial: 1,
            pending: BTreeMap::new(),
        }
    }

    /// Build a method call. Sends are the caller's job (the transport).
    #[must_use]
    pub fn method_call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &'static str,
        body: Vec<DbusValue>,
        no_reply: bool,
    ) -> OutgoingCall {
        let serial = self.next_serial;
        self.next_serial += 1;
        self.pending.insert(serial, member);
        let signature: String = body.iter().map(DbusValue::signature).collect();
        let msg = DbusMessage {
            kind: MessageType::MethodCall,
            flags: u8::from(no_reply),
            serial,
            path: Some(path.to_owned()),
            interface: Some(interface.to_owned()),
            member: Some(member.to_owned()),
            error_name: None,
            reply_serial: None,
            destination: Some(destination.to_owned()),
            sender: None,
            signature,
            body,
            unix_fds: 0,
        };
        OutgoingCall {
            serial,
            bytes: msg.marshal(),
            fds: Vec::new(),
            member,
        }
    }

    /// Build a signal to emit (rare for a session client).
    #[must_use]
    pub fn signal(
        &mut self,
        path: &str,
        interface: &str,
        member: &'static str,
        body: Vec<DbusValue>,
    ) -> OutgoingCall {
        let serial = self.next_serial;
        self.next_serial += 1;
        let signature: String = body.iter().map(DbusValue::signature).collect();
        let msg = DbusMessage {
            kind: MessageType::Signal,
            flags: 0,
            serial,
            path: Some(path.to_owned()),
            interface: Some(interface.to_owned()),
            member: Some(member.to_owned()),
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            signature,
            body,
            unix_fds: 0,
        };
        OutgoingCall {
            serial,
            bytes: msg.marshal(),
            fds: Vec::new(),
            member,
        }
    }

    /// Route one received message to a typed event (reply matching by
    /// serial; unknown serials are dropped — the bus never invents
    /// replies).
    #[must_use]
    pub fn on_message(&mut self, msg: &DbusMessage) -> Option<DbusEvent> {
        match msg.kind {
            MessageType::MethodReturn => {
                let reply_to = msg.reply_serial?;
                let member = self.pending.remove(&reply_to)?;
                Some(DbusEvent::Reply {
                    serial: msg.serial,
                    reply_to,
                    member,
                    values: msg.body.clone(),
                })
            }
            MessageType::Error => {
                let reply_to = msg.reply_serial?;
                let member = self.pending.remove(&reply_to)?;
                Some(DbusEvent::Error {
                    reply_to,
                    member,
                    name: msg.error_name.clone().unwrap_or_default(),
                    values: msg.body.clone(),
                })
            }
            MessageType::Signal => Some(DbusEvent::Signal {
                interface: msg.interface.clone().unwrap_or_default(),
                member: msg.member.clone().unwrap_or_default(),
                path: msg.path.clone().unwrap_or_default(),
                values: msg.body.clone(),
            }),
            MessageType::MethodCall => None, // a session client takes no calls
        }
    }

    /// Pending calls still awaiting replies.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Parse helper for transport implementations.
    ///
    /// # Errors
    ///
    /// [`DbusError`] when the bytes are not a well-formed message.
    pub fn parse(bytes: &[u8]) -> Result<DbusMessage, DbusError> {
        parse_message(bytes)
    }
}

/// The SASL EXTERNAL auth preamble a unix-socket transport writes
/// before `BEGIN` (hex-encoded uid; the peer credential is the proof).
#[must_use]
pub fn auth_external_bytes(uid: u32) -> Vec<u8> {
    let hex: String = uid.to_string().bytes().fold(String::new(), |mut acc, b| {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        acc.push(HEX[(b >> 4) as usize] as char);
        acc.push(HEX[(b & 0xf) as usize] as char);
        acc
    });
    format!("\0AUTH EXTERNAL {hex}\r\nBEGIN\r\n").into_bytes()
}
