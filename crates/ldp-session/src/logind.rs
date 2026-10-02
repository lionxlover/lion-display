//! The logind session client — TakeControl/TakeDevice, the pause and
//! resume device signals, lock/unlock, and sleep preparation.
//!
//! The state machine speaks through [`DbusConnection`] calls and maps
//! incoming signals to typed [`SessionEvent`]s. Device lifecycles are
//! tracked per (major, minor): a `pause` reason wants an explicit
//! `PauseDeviceComplete` ack; `force` releases immediately; `gone`
//! means the device vanished (release without ack).

use std::collections::BTreeMap;

use crate::conn::{DbusConnection, DbusEvent, OutgoingCall};
use crate::dbus::{DbusValue, MessageType};

/// Why a device paused (logind's reason string).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PauseReason {
    /// The device is paused but the client may ack completion.
    Pause,
    /// Paused by force; no ack is possible.
    Force,
    /// The device is gone (e.g. unplugged).
    Gone,
}

impl PauseReason {
    /// Parse logind's reason string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pause" => Some(Self::Pause),
            "force" => Some(Self::Force),
            "gone" => Some(Self::Gone),
            _ => None,
        }
    }
}

/// Typed session-layer events.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// `TakeControl` returned.
    Controlled,
    /// `TakeDevice` returned with its fd and inactive flag.
    DeviceLease {
        /// Device major.
        major: u32,
        /// Device minor.
        minor: u32,
        /// fd token from the reply.
        fd: u32,
        /// logind's "inactive" flag.
        inactive: bool,
    },
    /// `PauseDevice` signal.
    DevicePaused {
        /// Device major.
        major: u32,
        /// Device minor.
        minor: u32,
        /// Why.
        reason: PauseReason,
        /// Whether a `PauseDeviceComplete` ack is owed.
        needs_ack: bool,
    },
    /// `ResumeDevice` signal with the new fd.
    DeviceResumed {
        /// Device major.
        major: u32,
        /// Device minor.
        minor: u32,
        /// The new fd token.
        fd: u32,
    },
    /// `Lock` signal — the lock screen should come up.
    LockRequested,
    /// `Unlock` signal.
    UnlockRequested,
    /// `PrepareForSleep` — true = about to sleep, false = resumed.
    PrepareForSleep(bool),
    /// A call failed with a D-Bus error.
    CallFailed {
        /// Which call.
        member: &'static str,
        /// Error name.
        name: String,
    },
    /// A signal arrived that did not parse (kept for diagnostics).
    Malformed {
        /// The interface it claimed.
        interface: String,
        /// The member it claimed.
        member: String,
    },
}

const LOGIN1: &str = "org.freedesktop.login1";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";

/// The session client.
#[derive(Debug)]
pub struct LogindSession {
    conn: DbusConnection,
    path: String,
    devices: BTreeMap<(u32, u32), DeviceState>,
    /// Outstanding TakeDevice calls: serial -> (major, minor). The
    /// reply echoes neither, so the request is the only source.
    pending_devices: BTreeMap<u32, (u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeviceState {
    fd: u32,
    inactive: bool,
}

impl LogindSession {
    /// A session client for `path`
    /// (`/org/freedesktop/login1/session/<id>`).
    #[must_use]
    pub fn new(path: &str) -> Self {
        LogindSession {
            conn: DbusConnection::new(),
            path: path.to_owned(),
            devices: BTreeMap::new(),
            pending_devices: BTreeMap::new(),
        }
    }

    /// The session object path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// `TakeControl(force)`.
    #[must_use]
    pub fn take_control(&mut self, force: bool) -> OutgoingCall {
        self.conn.method_call(
            LOGIN1,
            &self.path,
            SESSION_IFACE,
            "TakeControl",
            vec![DbusValue::Bool(force)],
            false,
        )
    }

    /// `TakeDevice(major, minor)` — reply carries `(fd, inactive)`.
    #[must_use]
    pub fn take_device(&mut self, major: u32, minor: u32) -> OutgoingCall {
        let call = self.conn.method_call(
            LOGIN1,
            &self.path,
            SESSION_IFACE,
            "TakeDevice",
            vec![DbusValue::U32(major), DbusValue::U32(minor)],
            false,
        );
        self.pending_devices.insert(call.serial, (major, minor));
        call
    }

    /// `ReleaseDevice(major, minor)`.
    #[must_use]
    pub fn release_device(&mut self, major: u32, minor: u32) -> OutgoingCall {
        self.devices.remove(&(major, minor));
        self.conn.method_call(
            LOGIN1,
            &self.path,
            SESSION_IFACE,
            "ReleaseDevice",
            vec![DbusValue::U32(major), DbusValue::U32(minor)],
            true,
        )
    }

    /// `PauseDeviceComplete(major, minor)` — the ack for a `pause`.
    #[must_use]
    pub fn pause_device_complete(&mut self, major: u32, minor: u32) -> OutgoingCall {
        self.conn.method_call(
            LOGIN1,
            &self.path,
            SESSION_IFACE,
            "PauseDeviceComplete",
            vec![DbusValue::U32(major), DbusValue::U32(minor)],
            true,
        )
    }

    /// `Lock()` — request the lock screen (compositor side pokes).
    #[must_use]
    pub fn lock(&mut self) -> OutgoingCall {
        self.conn
            .method_call(LOGIN1, &self.path, SESSION_IFACE, "Lock", vec![], true)
    }

    /// `Unlock()`.
    #[must_use]
    pub fn unlock(&mut self) -> OutgoingCall {
        self.conn
            .method_call(LOGIN1, &self.path, SESSION_IFACE, "Unlock", vec![], true)
    }

    /// Route one received (parsed) message into typed events.
    #[must_use]
    pub fn on_message(&mut self, msg: &crate::dbus::DbusMessage) -> Vec<SessionEvent> {
        let Some(event) = self.conn.on_message(msg) else {
            return Vec::new();
        };
        match event {
            DbusEvent::Reply {
                reply_to,
                member,
                values,
                ..
            } => {
                vec![self.on_reply(reply_to, member, &values)]
            }
            DbusEvent::Error { member, name, .. } => {
                vec![SessionEvent::CallFailed { member, name }]
            }
            DbusEvent::Signal {
                interface,
                member,
                values,
                ..
            } => self.on_signal(interface, member, &values),
        }
    }

    fn on_reply(
        &mut self,
        reply_to: u32,
        member: &'static str,
        values: &[DbusValue],
    ) -> SessionEvent {
        match member {
            "TakeControl" => SessionEvent::Controlled,
            "TakeDevice" => {
                // Reply signature (hb): fd index + inactive flag.
                let Some((major, minor)) = self.pending_devices.remove(&reply_to) else {
                    return SessionEvent::CallFailed {
                        member,
                        name: "unmatched TakeDevice reply".to_owned(),
                    };
                };
                if let [DbusValue::Fd(fd), DbusValue::Bool(inactive)] = values {
                    self.devices.insert(
                        (major, minor),
                        DeviceState {
                            fd: *fd,
                            inactive: *inactive,
                        },
                    );
                    SessionEvent::DeviceLease {
                        major,
                        minor,
                        fd: *fd,
                        inactive: *inactive,
                    }
                } else {
                    SessionEvent::CallFailed {
                        member,
                        name: "bad reply signature".to_owned(),
                    }
                }
            }
            other => SessionEvent::CallFailed {
                member: other,
                name: "unexpected reply".to_owned(),
            },
        }
    }

    fn on_signal(
        &mut self,
        interface: String,
        member: String,
        values: &[DbusValue],
    ) -> Vec<SessionEvent> {
        if interface != SESSION_IFACE {
            return vec![SessionEvent::Malformed { interface, member }];
        }
        match member.as_str() {
            "PauseDevice" => {
                if let [DbusValue::U32(major), DbusValue::U32(minor), DbusValue::Str(reason), ..] =
                    values
                {
                    let Some(reason) = PauseReason::parse(reason) else {
                        return vec![SessionEvent::Malformed { interface, member }];
                    };
                    vec![SessionEvent::DevicePaused {
                        major: *major,
                        minor: *minor,
                        reason,
                        needs_ack: reason == PauseReason::Pause,
                    }]
                } else {
                    vec![SessionEvent::Malformed { interface, member }]
                }
            }
            "ResumeDevice" => {
                if let [DbusValue::U32(major), DbusValue::U32(minor), DbusValue::Fd(fd)] = values {
                    if let Some(state) = self.devices.get_mut(&(*major, *minor)) {
                        state.fd = *fd;
                    }
                    vec![SessionEvent::DeviceResumed {
                        major: *major,
                        minor: *minor,
                        fd: *fd,
                    }]
                } else {
                    vec![SessionEvent::Malformed { interface, member }]
                }
            }
            "Lock" => vec![SessionEvent::LockRequested],
            "Unlock" => vec![SessionEvent::UnlockRequested],
            "PrepareForSleep" => {
                if let [DbusValue::Bool(sleeping)] = values {
                    vec![SessionEvent::PrepareForSleep(*sleeping)]
                } else {
                    vec![SessionEvent::Malformed { interface, member }]
                }
            }
            _ => vec![SessionEvent::Malformed { interface, member }],
        }
    }

    /// Register a device lease's outcome (call after `TakeDevice`'s
    /// reply; the reply itself does not echo the device numbers).
    pub fn register_lease(&mut self, major: u32, minor: u32, fd: u32, inactive: bool) {
        self.devices
            .insert((major, minor), DeviceState { fd, inactive });
    }

    /// Held device leases.
    #[must_use]
    pub fn devices(&self) -> Vec<(u32, u32, u32, bool)> {
        self.devices
            .iter()
            .map(|((major, minor), s)| (*major, *minor, s.fd, s.inactive))
            .collect()
    }

    /// Build a reply message (test harness / transport support).
    #[must_use]
    pub fn reply_for(call: &OutgoingCall, values: Vec<DbusValue>, serial: u32) -> Vec<u8> {
        let signature: String = values.iter().map(DbusValue::signature).collect();
        crate::dbus::DbusMessage {
            kind: MessageType::MethodReturn,
            flags: 0,
            serial,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: Some(call.serial),
            destination: None,
            sender: Some("org.freedesktop.DBus".to_owned()),
            signature,
            body: values,
            unix_fds: 0,
        }
        .marshal()
    }

    /// Build a signal message (test harness / transport support).
    #[must_use]
    pub fn signal_bytes(
        path: &str,
        interface: &str,
        member: &str,
        values: Vec<DbusValue>,
        serial: u32,
    ) -> Vec<u8> {
        let signature: String = values.iter().map(DbusValue::signature).collect();
        crate::dbus::DbusMessage {
            kind: MessageType::Signal,
            flags: 0,
            serial,
            path: Some(path.to_owned()),
            interface: Some(interface.to_owned()),
            member: Some(member.to_owned()),
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: Some(":1.42".to_owned()),
            signature,
            body: values,
            unix_fds: 0,
        }
        .marshal()
    }

    /// Parse raw bytes (test harness support).
    pub fn parse(bytes: &[u8]) -> Option<crate::dbus::DbusMessage> {
        crate::dbus::parse_message(bytes).ok()
    }
}
