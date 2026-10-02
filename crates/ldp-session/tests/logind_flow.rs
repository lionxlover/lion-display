//! The logind session flow over a scripted transport: the full
//! TakeControl → TakeDevice → pause/resume (VT switch) → lock/unlock →
//! sleep cycle, byte-marshaled end to end (every event the harness
//! feeds the session is real marshaled D-Bus, parsed by the codec).

use ldp_session::conn::{DbusTransport, OutgoingCall};
use ldp_session::dbus::{parse_message, DbusError, DbusValue, MessageType};
use ldp_session::logind::{LogindSession, PauseReason, SessionEvent};
use ldp_session::vt::{VtAction, VtState, VtSwitcher};

const SESSION_PATH: &str = "/org/freedesktop/login1/session/c2";

/// A transport that records outgoing calls and plays a scripted
/// incoming queue.
#[derive(Default)]
struct ScriptedTransport {
    sent: Vec<(u32, &'static str)>,
    inbox: Vec<Vec<u8>>,
}

impl ScriptedTransport {
    fn queue_reply(&mut self, call: &OutgoingCall, values: Vec<DbusValue>, serial: u32) {
        let signature: String = values.iter().map(DbusValue::signature).collect();
        self.inbox.push(
            ldp_session::dbus::DbusMessage {
                kind: MessageType::MethodReturn,
                flags: 0,
                serial,
                path: None,
                interface: None,
                member: None,
                error_name: None,
                reply_serial: Some(call.serial),
                destination: None,
                sender: Some(":1.9".to_owned()),
                signature,
                body: values,
                unix_fds: 0,
            }
            .marshal(),
        );
    }

    fn queue_signal(&mut self, member: &str, values: Vec<DbusValue>, serial: u32) {
        self.inbox.push(
            ldp_session::dbus::DbusMessage {
                kind: MessageType::Signal,
                flags: 0,
                serial,
                path: Some(SESSION_PATH.to_owned()),
                interface: Some("org.freedesktop.login1.Session".to_owned()),
                member: Some(member.to_owned()),
                error_name: None,
                reply_serial: None,
                destination: None,
                sender: Some(":1.9".to_owned()),
                signature: values.iter().map(DbusValue::signature).collect(),
                body: values,
                unix_fds: 0,
            }
            .marshal(),
        );
    }
}

impl ScriptedTransport {
    /// Record an outgoing call's serial + member without leaking.
    fn record(&mut self, msg: &ldp_session::dbus::DbusMessage) {
        let member: &'static str = match msg.member.as_deref() {
            Some("TakeControl") => "TakeControl",
            Some("TakeDevice") => "TakeDevice",
            Some("ReleaseDevice") => "ReleaseDevice",
            Some("PauseDeviceComplete") => "PauseDeviceComplete",
            Some("Lock") => "Lock",
            Some("Unlock") => "Unlock",
            _ => "?",
        };
        self.sent.push((msg.serial, member));
    }
}

impl DbusTransport for ScriptedTransport {
    fn send(&mut self, bytes: &[u8], _fds: &[u32]) -> Result<(), DbusError> {
        let msg = parse_message(bytes)?;
        self.record(&msg);
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<ldp_session::dbus::DbusMessage>, DbusError> {
        match self.inbox.first() {
            Some(bytes) => {
                let msg = parse_message(bytes)?;
                self.inbox.remove(0);
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }
}

fn drain(session: &mut LogindSession, transport: &mut ScriptedTransport) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Ok(Some(msg)) = transport.recv() {
        events.extend(session.on_message(&msg));
    }
    events
}

fn send(transport: &mut ScriptedTransport, call: &OutgoingCall) {
    transport.send(&call.bytes, &call.fds).unwrap();
}

/// The scripted full-cycle walkthrough: one linear scenario, all
/// assertions in sequence.
#[allow(clippy::too_many_lines)]
#[test]
fn full_session_lifecycle() {
    let mut session = LogindSession::new(SESSION_PATH);
    let mut transport = ScriptedTransport::default();
    let mut vt = VtSwitcher::new();

    // TakeControl(false) -> Controlled.
    let call = session.take_control(false);
    send(&mut transport, &call);
    let call = session.take_control(false);
    transport.queue_reply(&call, vec![], 100);
    send(&mut transport, &call);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::Controlled]
    );

    // TakeDevice(226, 0) -> (fd 3, inactive false).
    let call = session.take_device(226, 0);
    transport.queue_reply(&call, vec![DbusValue::Fd(3), DbusValue::Bool(false)], 101);
    send(&mut transport, &call);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::DeviceLease {
            major: 226,
            minor: 0,
            fd: 3,
            inactive: false
        }]
    );
    assert_eq!(session.devices(), vec![(226, 0, 3, false)]);

    // VT switch away: PauseDevice("pause") -> release + ack.
    transport.queue_signal(
        "PauseDevice",
        vec![
            DbusValue::U32(226),
            DbusValue::U32(0),
            DbusValue::Str("pause".into()),
            DbusValue::Str("vt".into()),
        ],
        102,
    );
    let events = drain(&mut session, &mut transport);
    assert_eq!(
        events,
        vec![SessionEvent::DevicePaused {
            major: 226,
            minor: 0,
            reason: PauseReason::Pause,
            needs_ack: true
        }]
    );
    let actions: Vec<Vec<VtAction>> = events.iter().map(|e| vt.on_event(e)).collect();
    assert_eq!(
        actions[0],
        vec![
            VtAction::ReleaseDrmMaster,
            VtAction::AckPause {
                major: 226,
                minor: 0
            },
        ]
    );
    assert_eq!(vt.state(), VtState::Pausing);
    let ack = session.pause_device_complete(226, 0);
    send(&mut transport, &ack);
    vt.switch_away_complete();

    // VT switch back: ResumeDevice with the fresh fd.
    transport.queue_signal(
        "ResumeDevice",
        vec![DbusValue::U32(226), DbusValue::U32(0), DbusValue::Fd(9)],
        103,
    );
    let events = drain(&mut session, &mut transport);
    assert_eq!(
        events,
        vec![SessionEvent::DeviceResumed {
            major: 226,
            minor: 0,
            fd: 9
        }]
    );
    assert_eq!(
        vt.on_event(&events[0]),
        vec![VtAction::ReacquireDevice {
            major: 226,
            minor: 0,
            fd: 9
        }]
    );
    vt.switch_back_complete();
    assert_eq!(vt.state(), VtState::Active);
    // The lease table tracks the fresh fd.
    assert_eq!(session.devices(), vec![(226, 0, 9, false)]);

    // Lock, unlock.
    transport.queue_signal("Lock", vec![], 104);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::LockRequested]
    );
    transport.queue_signal("Unlock", vec![], 105);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::UnlockRequested]
    );

    // Sleep cycle: PrepareForSleep(true), then (false).
    transport.queue_signal("PrepareForSleep", vec![DbusValue::Bool(true)], 106);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::PrepareForSleep(true)]
    );
    transport.queue_signal("PrepareForSleep", vec![DbusValue::Bool(false)], 107);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::PrepareForSleep(false)]
    );

    // The call log: every method call was marshaled and sent.
    let members: Vec<&str> = transport.sent.iter().map(|(_, m)| *m).collect();
    assert_eq!(
        members,
        vec![
            "TakeControl",
            "TakeControl",
            "TakeDevice",
            "PauseDeviceComplete",
        ]
    );
}

#[test]
fn force_and_gone_pauses_have_no_ack() {
    let mut session = LogindSession::new(SESSION_PATH);
    let mut transport = ScriptedTransport::default();
    let mut vt = VtSwitcher::new();
    for reason in ["force", "gone"] {
        transport.queue_signal(
            "PauseDevice",
            vec![
                DbusValue::U32(226),
                DbusValue::U32(0),
                DbusValue::Str(reason.into()),
                DbusValue::Str("seat".into()),
            ],
            200,
        );
        let events = drain(&mut session, &mut transport);
        let needs_ack = reason == "pause";
        assert_eq!(
            events,
            vec![SessionEvent::DevicePaused {
                major: 226,
                minor: 0,
                reason: PauseReason::parse(reason).unwrap(),
                needs_ack
            }]
        );
        // Force: release only, no ack action.
        let actions = vt.on_event(&events[0]);
        assert_eq!(actions, vec![VtAction::ReleaseDrmMaster]);
        vt.switch_away_complete();
        // Switch back before the next iteration.
        transport.queue_signal(
            "ResumeDevice",
            vec![DbusValue::U32(226), DbusValue::U32(0), DbusValue::Fd(5)],
            201,
        );
        let events = drain(&mut session, &mut transport);
        assert!(!vt.on_event(&events[0]).is_empty());
        vt.switch_back_complete();
    }
    assert_eq!(vt.state(), VtState::Active);
}

#[test]
fn error_replies_surface_typed() {
    let mut session = LogindSession::new(SESSION_PATH);
    let mut transport = ScriptedTransport::default();
    let call = session.take_control(true);
    transport.inbox.push(
        ldp_session::dbus::DbusMessage {
            kind: MessageType::Error,
            flags: 0,
            serial: 300,
            path: None,
            interface: None,
            member: None,
            error_name: Some("org.freedesktop.DBus.Error.AccessDenied".to_owned()),
            reply_serial: Some(call.serial),
            destination: None,
            sender: Some(":1.9".to_owned()),
            signature: "s".to_owned(),
            body: vec![DbusValue::Str("no".into())],
            unix_fds: 0,
        }
        .marshal(),
    );
    send(&mut transport, &call);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::CallFailed {
            member: "TakeControl",
            name: "org.freedesktop.DBus.Error.AccessDenied".to_owned(),
        }]
    );
}

#[test]
fn malformed_signals_report_and_do_not_derail() {
    let mut session = LogindSession::new(SESSION_PATH);
    let mut transport = ScriptedTransport::default();
    // A PauseDevice with a bogus body: typed as Malformed, the machine
    // state untouched.
    transport.queue_signal("PauseDevice", vec![], 400);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::Malformed {
            interface: "org.freedesktop.login1.Session".to_owned(),
            member: "PauseDevice".to_owned(),
        }]
    );
    // An unknown-reason pause is malformed too.
    transport.queue_signal(
        "PauseDevice",
        vec![
            DbusValue::U32(226),
            DbusValue::U32(0),
            DbusValue::Str("weird".into()),
            DbusValue::Str("?".into()),
        ],
        401,
    );
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::Malformed {
            interface: "org.freedesktop.login1.Session".to_owned(),
            member: "PauseDevice".to_owned(),
        }]
    );
    // A foreign interface is not ours to interpret.
    transport.inbox.push(
        ldp_session::dbus::DbusMessage {
            kind: MessageType::Signal,
            flags: 0,
            serial: 402,
            path: Some("/org/other".to_owned()),
            interface: Some("org.example.Other".to_owned()),
            member: Some("Ping".to_owned()),
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            signature: String::new(),
            body: vec![],
            unix_fds: 0,
        }
        .marshal(),
    );
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::Malformed {
            interface: "org.example.Other".to_owned(),
            member: "Ping".to_owned(),
        }]
    );
    // The session still works afterwards.
    let call = session.take_device(1, 2);
    transport.queue_reply(&call, vec![DbusValue::Fd(1), DbusValue::Bool(true)], 403);
    send(&mut transport, &call);
    assert_eq!(
        drain(&mut session, &mut transport),
        vec![SessionEvent::DeviceLease {
            major: 1,
            minor: 2,
            fd: 1,
            inactive: true
        }]
    );
}

#[test]
fn release_device_drops_the_lease() {
    let mut session = LogindSession::new(SESSION_PATH);
    let mut transport = ScriptedTransport::default();
    let call = session.take_device(226, 0);
    transport.queue_reply(&call, vec![DbusValue::Fd(3), DbusValue::Bool(false)], 500);
    send(&mut transport, &call);
    drain(&mut session, &mut transport);
    let release = session.release_device(226, 0);
    send(&mut transport, &release);
    assert!(session.devices().is_empty());
}
