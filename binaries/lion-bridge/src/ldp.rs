//! The bridge's own LDP client connection — raw framing over the
//! transport, the hello/welcome handshake, and the inbound event
//! router that feeds the bridge drivers.
//!
//! The drivers emit fully-formed [`Message`] values with their own
//! fixed object-id plan (bootstrap ids 1–6, per-export ids from 10
//! upward for the Wayland face, 7–13 for the rootful X11 face), so
//! this link speaks the wire directly instead of routing through
//! `ldp-client`'s proxy machinery: every driver message is
//! schema-built already, and every inbound event is routed by the
//! same fixed id plan the drivers documented.
//!
//! The link additionally owns two factory objects the drivers leave
//! to the process layer: the seat's pointer and keyboard proxies
//! (ids [`POINTER_ID`]/[`KEYBOARD_ID`], far above any driver's export
//! range), because LDP input events target per-device objects the
//! drivers never minted.

#![forbid(unsafe_code)]

use ldp_core::error::LdpError;
use ldp_core::ids::CONNECTION_OBJECT_ID;
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::generated::{core, input, shell};
use ldp_protocol::{decode, Message, ValidationMode};
use ldp_transport::fd::FdList;
use ldp_transport::reader::FramedReader;
use ldp_transport::stream::TransportStream;
use ldp_transport::writer::FramedWriter;
use ldp_transport::{is_would_block, UnixAddr};

/// The seat's pointer proxy id (the link's own factory object).
pub const POINTER_ID: u32 = 0x7000_0000;
/// The seat's keyboard proxy id (the link's own factory object).
pub const KEYBOARD_ID: u32 = 0x7000_0001;
/// The bound seat object (both drivers bind it at id 5).
const SEAT_ID: u32 = 5;

/// One inbound event, routed by the link.
#[derive(Clone, Debug)]
pub enum LdpEvent {
    /// `toplevel.configure` — the foreign client must resize.
    Configure {
        /// The toplevel the driver minted.
        toplevel: u32,
        /// The offered width.
        width: i32,
        /// The offered height.
        height: i32,
    },
    /// `toplevel.close`.
    Close {
        /// The toplevel the driver minted.
        toplevel: u32,
    },
    /// `surface.presented` — the frame landed; the driver ticks.
    Presented,
    /// A pointer enter or motion (surface-local).
    PointerMotion {
        /// The surface the driver exported.
        surface: u32,
        /// Surface-local x.
        x: i32,
        /// Surface-local y.
        y: i32,
    },
    /// A pointer leave (focus lost).
    PointerLeave,
    /// A pointer button.
    PointerButton {
        /// The LDP button code.
        button: u32,
        /// Pressed or released.
        pressed: bool,
    },
    /// A keyboard enter (focus gained, with the pressed-key set).
    KeyboardEnter {
        /// The surface the driver exported.
        surface: u32,
        /// The currently pressed keys (evdev codes).
        keys: Vec<u32>,
    },
    /// A keyboard leave.
    KeyboardLeave,
    /// A key event.
    Key {
        /// The evdev keycode.
        keycode: u32,
        /// Pressed or released.
        pressed: bool,
    },
    /// `popup.configure` — the constraint solver's placement proposal
    /// (the rootless X11 face acks it; the X geometry stays the X
    /// truth, the display follows the solver).
    PopupConfigure {
        /// The popup object the proposal targets.
        popup: u32,
        /// The proposal's serial (the ack references it).
        serial: u32,
    },
    /// `popup.done` — the popup was dismissed server-side.
    PopupDone {
        /// The dismissed popup object.
        popup: u32,
    },
}

/// Why a link died.
#[derive(Clone, Debug)]
pub enum LinkDeath {
    /// The transport failed (EOF, framing, socket).
    Transport(String),
    /// The server ended the session (`connection.destroyed`).
    Destroyed,
    /// The server rejected the session (`connection.error`) — the
    /// diagnostic the operator reads.
    Server(String),
}

/// The bridge's LDP connection.
pub struct LdpLink {
    stream: TransportStream,
    reader: FramedReader,
    writer: FramedWriter<ldp_transport::backpressure::NoHooks>,
    limits: Limits,
    /// The ids this link treats as toplevels (driver-minted).
    toplevels: Vec<u32>,
    /// The ids this link treats as export surfaces (driver-minted).
    surfaces: Vec<u32>,
    /// The ids this link treats as popups (the rootless driver's OR
    /// windows).
    popups: Vec<u32>,
    /// The keepalive cookie counter (`connection.sync`).
    sync_cookie: u32,
    welcome: bool,
    dead: Option<LinkDeath>,
}

impl LdpLink {
    /// Connect (nonblocking) and stage the handshake: the caller
    /// supplies the driver's bootstrap messages (hello first), this
    /// constructor sends them, then pumps until `welcome` arrives.
    /// A bootstrap message carrying an FD argument rides
    /// `bootstrap_fd` (the rootful X11 face's `create_pool`).
    ///
    /// # Errors
    ///
    /// [`LdpError`] on connect or transport failure; a missing
    /// welcome (timeout) surfaces as a logic error naming it.
    pub fn connect(
        addr: &UnixAddr,
        bootstrap: &[Message],
        bootstrap_fd: Option<std::os::fd::OwnedFd>,
    ) -> Result<LdpLink, LdpError> {
        let mut stream = TransportStream::connect(addr)?;
        stream.set_nonblocking(true)?;
        let mut link = LdpLink {
            stream,
            reader: FramedReader::new(Limits::default()),
            writer: FramedWriter::without_hooks(Limits::default()),
            limits: Limits::default(),
            toplevels: Vec::new(),
            surfaces: Vec::new(),
            popups: Vec::new(),
            sync_cookie: 0,
            welcome: false,
            dead: None,
        };
        match bootstrap_fd {
            Some(fd) => link.send_with_fd(bootstrap, fd)?,
            None => link.send(bootstrap, &mut FdList::new())?,
        }
        // The handshake pump: welcome must arrive before anything the
        // drivers care about; the socket parks in poll between tries.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2_000);
        while !link.welcome {
            if std::time::Instant::now() > deadline {
                return Err(LdpError::TimedOut {
                    what: "bridge handshake waiting for connection.welcome",
                });
            }
            match link.recv_one() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    crate::sys::poll(
                        &[crate::sys::Interest {
                            fd: link.stream.raw_fd(),
                            read: true,
                            write: false,
                        }],
                        100,
                    )?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(link)
    }

    /// Whether the server's welcome arrived (the link is live).
    #[must_use]
    pub fn is_welcomed(&self) -> bool {
        self.welcome
    }

    /// How the link died, once it did.
    #[must_use]
    pub fn death(&self) -> Option<&LinkDeath> {
        self.dead.as_ref()
    }

    /// The transport's raw descriptor (poll registration).
    #[must_use]
    pub fn fd(&self) -> i32 {
        self.stream.raw_fd()
    }

    /// Whether outbound bytes are still queued (poll write-interest).
    #[must_use]
    pub fn wants_write(&self) -> bool {
        self.writer.pending_bytes() > 0
    }

    /// Register a driver-minted toplevel id for configure/close
    /// routing (called when the driver mints one — the X11 driver's
    /// fixed id, each Wayland export's id, each rootless window's
    /// dynamic id). Idempotent: the rootless face re-syncs its watch
    /// set every turn.
    pub fn watch_toplevel(&mut self, id: u32) {
        if !self.toplevels.contains(&id) {
            self.toplevels.push(id);
        }
    }

    /// Register a driver-minted surface id for presented routing.
    /// Idempotent (the rootless watch sync).
    pub fn watch_surface(&mut self, id: u32) {
        if !self.surfaces.contains(&id) {
            self.surfaces.push(id);
        }
    }

    /// Register a driver-minted popup id for configure/done routing
    /// (the rootless driver's OR windows). Idempotent.
    pub fn watch_popup(&mut self, id: u32) {
        if !self.popups.contains(&id) {
            self.popups.push(id);
        }
    }

    /// The link's own factory pair: `seat.get_pointer` and
    /// `seat.get_keyboard` over the bound seat — emitted after the
    /// welcome so input events have per-device objects to target.
    #[must_use]
    pub fn seat_factories(&self) -> Vec<Message> {
        let pointer = Message::new(SEAT_ID, input::seat::request::GET_POINTER).arg(Value::NewId(
            ldp_core::ids::ObjectId::client(POINTER_ID).expect("the link's pointer id is in range"),
        ));
        let keyboard = Message::new(SEAT_ID, input::seat::request::GET_KEYBOARD).arg(Value::NewId(
            ldp_core::ids::ObjectId::client(KEYBOARD_ID)
                .expect("the link's keyboard id is in range"),
        ));
        vec![pointer, keyboard]
    }

    /// The quiescent keepalive: `connection.sync` — the protocol's
    /// own polling request ("never advances protocol state"). The
    /// compositor parks cross-client events (input among them) for
    /// the owner's next message; a bridge that only *reacts* (X
    /// drawing, presentation ticks) would never poll them out. One
    /// sync per idle turn keeps the parked events flowing — the
    /// keepalive cadence, a few 32-byte frames a second.
    ///
    /// # Errors
    ///
    /// [`LdpError`] on transport failure.
    pub fn sync(&mut self) -> Result<(), LdpError> {
        self.sync_cookie = self.sync_cookie.wrapping_add(1);
        let sync = Message::new(CONNECTION_OBJECT_ID, core::connection::request::SYNC)
            .arg(Value::Uint32(self.sync_cookie));
        self.send(
            std::slice::from_ref(&sync),
            &mut ldp_transport::fd::FdList::new(),
        )
    }

    /// Send messages (any FD arguments must already ride `fds`).
    ///
    /// # Errors
    ///
    /// [`LdpError`] on transport failure or a congestion ceiling
    /// breach (the drivers' volumes never approach it).
    pub fn send(&mut self, msgs: &[Message], fds: &mut FdList) -> Result<(), LdpError> {
        if self.dead.is_some() {
            return Err(LdpError::Logic {
                what: "the bridge link is already dead",
            });
        }
        for m in msgs {
            let bytes = m.encode(&self.limits)?;
            let outcome = self.writer.send_msg(&mut self.stream, &bytes, fds)?;
            let _ = outcome; // Sent or Congested: both leave the queue owned
        }
        self.writer.flush(&mut self.stream)?;
        Ok(())
    }

    /// Send one message batch with a single pool descriptor riding
    /// argument 0 (the `shm.create_pool` flow).
    ///
    /// # Errors
    ///
    /// [`LdpError`] on transport failure.
    pub fn send_with_fd(
        &mut self,
        msgs: &[Message],
        fd: std::os::fd::OwnedFd,
    ) -> Result<(), LdpError> {
        // The batch splits at fd-carrying messages: each rides
        // exactly one descriptor, the rest ride none (the framing
        // validates both sides of every message — one shared list
        // would charge the first message for the whole batch).
        let mut plain: Vec<Message> = Vec::new();
        for m in msgs {
            let carries_fd = m.args.iter().any(|a| matches!(a, Value::Fd(_)));
            if carries_fd {
                if !plain.is_empty() {
                    self.send(&plain, &mut FdList::new())?;
                    plain.clear();
                }
                let mut fds = FdList::new();
                fds.push(
                    fd.try_clone()
                        .map_err(|e| LdpError::Io(std::sync::Arc::new(e)))?,
                );
                self.send(std::slice::from_ref(m), &mut fds)?;
            } else {
                plain.push(m.clone());
            }
        }
        if !plain.is_empty() {
            self.send(&plain, &mut FdList::new())?;
        }
        Ok(())
    }

    /// Push queued bytes toward the socket (writability).
    ///
    /// # Errors
    ///
    /// [`LdpError`] on transport failure.
    pub fn flush(&mut self) -> Result<(), LdpError> {
        self.writer.flush(&mut self.stream)?;
        Ok(())
    }

    /// Read one inbound frame and route it; `Ok(None)` when nothing
    /// is readable (the nonblocking park).
    ///
    /// # Errors
    ///
    /// [`LdpError`] on transport or codec failure (the link is dead —
    /// the caller tears the session down).
    pub fn recv_one(&mut self) -> Result<Option<LdpEvent>, LdpError> {
        if self.dead.is_some() {
            return Err(LdpError::Logic {
                what: "the bridge link is already dead",
            });
        }
        let frame = match self.reader.recv_msg(&mut self.stream) {
            Ok(f) => f,
            Err(e) => {
                if is_would_block(&e) && self.stream.is_nonblocking() {
                    return Ok(None);
                }
                self.dead = Some(LinkDeath::Transport(e.to_string()));
                return Err(e);
            }
        };
        let msg = decode(
            frame.message_bytes(),
            u32::from(frame.fd_count()),
            &self.limits,
            ValidationMode::Tolerant,
        )?;
        Ok(self.route(&msg))
    }

    /// Route one decoded event by the drivers' documented id plan.
    fn route(&mut self, msg: &Message) -> Option<LdpEvent> {
        // The connection object: welcome completes the handshake;
        // destroyed ends the session.
        if msg.object_id == CONNECTION_OBJECT_ID {
            match msg.opcode {
                core::connection::event::WELCOME => {
                    self.welcome = true;
                    None
                }
                core::connection::event::DESTROYED => {
                    self.dead = Some(LinkDeath::Destroyed);
                    None
                }
                core::connection::event::ERROR => {
                    // The wire shape: (code enum, object u32, message
                    // string) — the diagnostic verbatim.
                    let message = match msg.args.get(2) {
                        Some(Value::String(s)) => s.to_string(),
                        _ => "unnamed protocol error".to_owned(),
                    };
                    self.dead = Some(LinkDeath::Server(message));
                    None
                }
                _ => None,
            }
        } else if msg.object_id == POINTER_ID {
            match msg.opcode {
                input::pointer::event::ENTER | input::pointer::event::MOTION => {
                    Some(pointer_xy(msg))
                }
                input::pointer::event::LEAVE => Some(LdpEvent::PointerLeave),
                input::pointer::event::BUTTON => Some(LdpEvent::PointerButton {
                    button: uint_at(msg, 0),
                    pressed: enum_at(msg, 1) == 2,
                }),
                _ => None,
            }
        } else if msg.object_id == KEYBOARD_ID {
            match msg.opcode {
                input::keyboard::event::ENTER => Some(LdpEvent::KeyboardEnter {
                    surface: object_at(msg, 0),
                    keys: key_array(msg),
                }),
                input::keyboard::event::LEAVE => Some(LdpEvent::KeyboardLeave),
                input::keyboard::event::KEY => Some(LdpEvent::Key {
                    keycode: uint_at(msg, 0),
                    pressed: enum_at(msg, 1) == 2,
                }),
                _ => None,
            }
        } else if self.toplevels.contains(&msg.object_id) {
            match msg.opcode {
                shell::toplevel::event::CONFIGURE => Some(LdpEvent::Configure {
                    toplevel: msg.object_id,
                    width: int_at(msg, 2),
                    height: int_at(msg, 3),
                }),
                shell::toplevel::event::CLOSE => Some(LdpEvent::Close {
                    toplevel: msg.object_id,
                }),
                _ => None,
            }
        } else if self.popups.contains(&msg.object_id) {
            match msg.opcode {
                shell::popup::event::CONFIGURE => Some(LdpEvent::PopupConfigure {
                    popup: msg.object_id,
                    serial: uint_at(msg, 0),
                }),
                shell::popup::event::DONE => Some(LdpEvent::PopupDone {
                    popup: msg.object_id,
                }),
                _ => None,
            }
        } else if self.surfaces.contains(&msg.object_id) {
            match msg.opcode {
                core::surface::event::PRESENTED => Some(LdpEvent::Presented),
                _ => None,
            }
        } else {
            None
        }
    }
}

/// The pointer enter/motion arg lift. The two events carry different
/// shapes: `enter` is (surface, x, y) — the transition with the
/// landing position; `motion` is (x, y) — surface-local deltas of the
/// already-focused surface. Reading the first two args uniformly
/// would lift the *surface object* as the x (a latent v0.10.0 bug the
/// rootless session's coordinate proof caught: every enter arrived
/// at (0, 0)).
fn pointer_xy(msg: &Message) -> LdpEvent {
    let surface = object_at(msg, 0);
    let (x, y) = match msg.args.as_slice() {
        [Value::Object(_), Value::Float32(x), Value::Float32(y)] => (*x as i32, *y as i32),
        [Value::Float32(x), Value::Float32(y)] => (*x as i32, *y as i32),
        _ => (0, 0),
    };
    LdpEvent::PointerMotion { surface, x, y }
}

fn uint_at(msg: &Message, i: usize) -> u32 {
    match msg.args.get(i) {
        Some(Value::Uint32(v)) => *v,
        Some(Value::Enum(v)) => *v,
        _ => 0,
    }
}

fn int_at(msg: &Message, i: usize) -> i32 {
    match msg.args.get(i) {
        Some(Value::Int32(v)) => *v,
        Some(Value::Uint32(v)) => *v as i32,
        _ => 0,
    }
}

fn enum_at(msg: &Message, i: usize) -> u32 {
    uint_at(msg, i)
}

fn object_at(msg: &Message, i: usize) -> u32 {
    match msg.args.get(i) {
        Some(Value::Object(Some(id))) => id.as_u32(),
        _ => 0,
    }
}

fn key_array(msg: &Message) -> Vec<u32> {
    match msg.args.get(1) {
        Some(Value::Array { items, .. }) => items
            .iter()
            .filter_map(|p| match p {
                ldp_core::wire::Primitive::Uint32(v) => Some(*v),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_ids_are_legal_client_ids() {
        assert!(ldp_core::ids::ObjectId::client(POINTER_ID).is_some());
        assert!(ldp_core::ids::ObjectId::client(KEYBOARD_ID).is_some());
        // Far above any driver's export range (the Wayland driver
        // mints from 10 upward; the X11 driver's ids stop at 13).
        // Far above the drivers' export ranges (the check must not
        // fold to a constant for the reader's eye):
        assert_eq!(POINTER_ID, 0x7000_0000);
        assert_eq!(KEYBOARD_ID, 0x7000_0001);
    }

    #[test]
    fn link_death_is_reportative() {
        assert!(matches!(
            LinkDeath::Transport("eof".into()),
            LinkDeath::Transport(_)
        ));
    }
}
