//! The typed shell-event vocabulary and its wire encoding.
//!
//! [`ShellEvent`] mirrors `spec/shell.toml` event-for-event: one Rust
//! variant per wire event, the same argument order, the same value
//! domains. Encoding goes through the compiled schema registry
//! ([`ldp_protocol::REGISTRY`]): the variant names its interface and
//! event, the registry supplies the opcode, and [`ShellEvent::args`]
//! builds the wire values — so a typo in either place fails loudly at
//! encode time instead of drifting silently. The conformance suites
//! round-trip every emitted message through the codec and the strict
//! signature checker.

#![forbid(unsafe_code)]

use ldp_core::bitset::Bitset128;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::{Direction, Message, REGISTRY};

use crate::dialog::DialogState;
use crate::popup::Placement;
use crate::serial::Serial;
use crate::toplevel::Configure;

/// Every event the `ldp.shell` module defines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShellEvent {
    /// `shell.workspace_count` — spaces on the seat's view.
    WorkspaceCount {
        /// Number of spaces.
        count: u32,
    },
    /// `toplevel.configure` — the state proposal.
    ToplevelConfigure {
        /// The proposal serial.
        serial: u32,
        /// State flags.
        states: Bitset128,
        /// Content width (0 = client chooses).
        width: u32,
        /// Content height.
        height: u32,
        /// Left inset.
        inset_left: i32,
        /// Top inset.
        inset_top: i32,
        /// Right inset.
        inset_right: i32,
        /// Bottom inset.
        inset_bottom: i32,
        /// The space the window is on.
        workspace: u32,
        /// The primary output.
        output: Option<ObjectId>,
    },
    /// `toplevel.close` — the user asked to close.
    ToplevelClose,
    /// `toplevel.workspace_changed` — the window moved spaces.
    WorkspaceChanged {
        /// The new space.
        workspace: u32,
    },
    /// `popup.configure` — the placement proposal.
    PopupConfigure {
        /// The proposal serial.
        serial: u32,
        /// Placement x (parent coords).
        x: i32,
        /// Placement y.
        y: i32,
        /// Placement width.
        width: u32,
        /// Placement height.
        height: u32,
    },
    /// `popup.done` — dismissed server-side.
    PopupDone,
    /// `dialog.configure` — the size proposal.
    DialogConfigure {
        /// The proposal serial.
        serial: u32,
        /// Content width.
        width: u32,
        /// Content height.
        height: u32,
    },
    /// `dialog.close` — the user asked to close.
    DialogClose,
}

impl ShellEvent {
    /// Build a [`ShellEvent::ToplevelConfigure`] from a machine
    /// proposal.
    #[must_use]
    pub fn toplevel_configure(c: &Configure) -> ShellEvent {
        ShellEvent::ToplevelConfigure {
            serial: c.serial.0,
            states: c.states.0,
            width: c.width,
            height: c.height,
            inset_left: c.insets.left as i32,
            inset_top: c.insets.top as i32,
            inset_right: c.insets.right as i32,
            inset_bottom: c.insets.bottom as i32,
            workspace: c.workspace,
            output: c.output,
        }
    }

    /// Build a [`ShellEvent::PopupConfigure`] from a solved placement.
    #[must_use]
    pub fn popup_configure(serial: Serial, p: &Placement) -> ShellEvent {
        ShellEvent::PopupConfigure {
            serial: serial.0,
            x: p.x,
            y: p.y,
            width: p.width,
            height: p.height,
        }
    }

    /// Build a [`ShellEvent::DialogConfigure`] from a machine
    /// proposal.
    #[must_use]
    pub fn dialog_configure(d: &DialogState) -> ShellEvent {
        ShellEvent::DialogConfigure {
            serial: d.serial.0,
            width: d.width,
            height: d.height,
        }
    }

    /// The emitting interface (FQ name).
    #[must_use]
    pub const fn interface(&self) -> &'static str {
        match self {
            ShellEvent::WorkspaceCount { .. } => "ldp.shell.shell",
            ShellEvent::ToplevelConfigure { .. }
            | ShellEvent::ToplevelClose
            | ShellEvent::WorkspaceChanged { .. } => "ldp.shell.toplevel",
            ShellEvent::PopupConfigure { .. } | ShellEvent::PopupDone => "ldp.shell.popup",
            ShellEvent::DialogConfigure { .. } | ShellEvent::DialogClose => "ldp.shell.dialog",
        }
    }

    /// The event name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            ShellEvent::WorkspaceCount { .. } => "workspace_count",
            ShellEvent::ToplevelConfigure { .. }
            | ShellEvent::PopupConfigure { .. }
            | ShellEvent::DialogConfigure { .. } => "configure",
            ShellEvent::ToplevelClose | ShellEvent::DialogClose => "close",
            ShellEvent::WorkspaceChanged { .. } => "workspace_changed",
            ShellEvent::PopupDone => "done",
        }
    }

    /// The wire arguments, in spec order.
    #[must_use]
    pub fn args(&self) -> Vec<Value> {
        fn u(v: u32) -> Value {
            Value::Uint32(v)
        }
        fn i(v: i32) -> Value {
            Value::Int32(v)
        }
        match self {
            ShellEvent::WorkspaceCount { count } => vec![u(*count)],
            ShellEvent::ToplevelConfigure {
                serial,
                states,
                width,
                height,
                inset_left,
                inset_top,
                inset_right,
                inset_bottom,
                workspace,
                output,
            } => vec![
                u(*serial),
                Value::Bitset(*states),
                u(*width),
                u(*height),
                i(*inset_left),
                i(*inset_top),
                i(*inset_right),
                i(*inset_bottom),
                u(*workspace),
                Value::Object(*output),
            ],
            ShellEvent::ToplevelClose | ShellEvent::PopupDone | ShellEvent::DialogClose => {
                vec![]
            }
            ShellEvent::WorkspaceChanged { workspace } => vec![u(*workspace)],
            ShellEvent::PopupConfigure {
                serial,
                x,
                y,
                width,
                height,
            } => vec![u(*serial), i(*x), i(*y), u(*width), u(*height)],
            ShellEvent::DialogConfigure {
                serial,
                width,
                height,
            } => vec![u(*serial), u(*width), u(*height)],
        }
    }

    /// Encode against the compiled registry: a [`Message`] addressed
    /// to `object`, opcode resolved from the schema.
    ///
    /// # Panics
    /// When the event's `(interface, name)` pair does not exist in the
    /// compiled spec — a build-time consistency break, not a runtime
    /// condition.
    #[must_use]
    pub fn to_message(&self, object: ObjectId) -> Message {
        let iface = REGISTRY
            .interface(self.interface())
            .unwrap_or_else(|| panic!("interface {} missing from schema", self.interface()));
        let op = iface
            .events
            .iter()
            .find(|e| e.name == self.name())
            .unwrap_or_else(|| {
                panic!(
                    "event {}.{} missing from schema",
                    self.interface(),
                    self.name()
                )
            });
        let mut m = Message::new(object.as_u32(), op.opcode);
        for v in self.args() {
            m = m.arg(v);
        }
        m
    }
}

/// The direction token re-exported for doc cross-references.
#[allow(unused_imports)]
use Direction as _DirectionRef;

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_protocol::generated::shell;

    #[test]
    fn opcodes_resolve_against_the_compiled_schema() {
        // The registry must carry every interface this vocabulary
        // addresses, with the exact event names.
        for iface in [
            "ldp.shell.shell",
            "ldp.shell.toplevel",
            "ldp.shell.popup",
            "ldp.shell.dialog",
        ] {
            assert!(REGISTRY.interface(iface).is_some(), "{iface} missing");
        }
        // Opcode pinning against the generated constants.
        let obj = ObjectId::from_wire(0x42);
        let e = ShellEvent::WorkspaceCount { count: 4 };
        assert_eq!(
            e.to_message(obj).opcode,
            shell::shell::event::WORKSPACE_COUNT
        );
        let e = ShellEvent::ToplevelClose;
        assert_eq!(e.to_message(obj).opcode, shell::toplevel::event::CLOSE);
        let e = ShellEvent::WorkspaceChanged { workspace: 2 };
        assert_eq!(
            e.to_message(obj).opcode,
            shell::toplevel::event::WORKSPACE_CHANGED
        );
        let e = ShellEvent::PopupDone;
        assert_eq!(e.to_message(obj).opcode, shell::popup::event::DONE);
        let e = ShellEvent::DialogClose;
        assert_eq!(e.to_message(obj).opcode, shell::dialog::event::CLOSE);
    }

    #[test]
    fn configure_variants_carry_the_full_argument_set() {
        let obj = ObjectId::from_wire(0x42);
        let out = ObjectId::from_wire(0x77);
        let e = ShellEvent::ToplevelConfigure {
            serial: 9,
            states: Bitset128::single(3),
            width: 800,
            height: 600,
            inset_left: 1,
            inset_top: 29,
            inset_right: 1,
            inset_bottom: 1,
            workspace: 1,
            output: Some(out),
        };
        let m = e.to_message(obj);
        assert_eq!(m.opcode, shell::toplevel::event::CONFIGURE);
        assert_eq!(m.args.len(), 10);
        assert_eq!(m.args[1], Value::Bitset(Bitset128::single(3)));
        assert_eq!(m.args[9], Value::Object(Some(out)));
        let e2 = ShellEvent::ToplevelConfigure {
            serial: 9,
            states: Bitset128::single(3),
            width: 800,
            height: 600,
            inset_left: 1,
            inset_top: 29,
            inset_right: 1,
            inset_bottom: 1,
            workspace: 1,
            output: None,
        };
        assert_eq!(e2.args()[9], Value::Object(None));

        let p = ShellEvent::PopupConfigure {
            serial: 3,
            x: -40,
            y: 12,
            width: 200,
            height: 100,
        };
        assert_eq!(
            p.args(),
            vec![
                Value::Uint32(3),
                Value::Int32(-40),
                Value::Int32(12),
                Value::Uint32(200),
                Value::Uint32(100),
            ]
        );

        let d = ShellEvent::DialogConfigure {
            serial: 5,
            width: 400,
            height: 300,
        };
        assert_eq!(
            d.args(),
            vec![Value::Uint32(5), Value::Uint32(400), Value::Uint32(300),]
        );
    }

    #[test]
    fn constructors_build_from_machine_outputs() {
        let c = Configure {
            serial: Serial(2),
            states: crate::toplevel::ToplevelStates::build(false, false, false, true, false, false),
            width: 640,
            height: 480,
            insets: crate::ssd::Insets {
                left: 1,
                top: 29,
                right: 1,
                bottom: 1,
            },
            workspace: 0,
            output: Some(ObjectId::from_wire(1)),
        };
        match ShellEvent::toplevel_configure(&c) {
            ShellEvent::ToplevelConfigure {
                serial,
                states,
                width,
                height,
                inset_top,
                ..
            } => {
                assert_eq!(serial, 2);
                assert!(states.test(3)); // activated
                assert_eq!((width, height), (640, 480));
                assert_eq!(inset_top, 29);
            }
            _ => panic!("wrong variant"),
        }
        let pl = Placement {
            x: 100,
            y: 200,
            width: 50,
            height: 60,
        };
        match ShellEvent::popup_configure(Serial(7), &pl) {
            ShellEvent::PopupConfigure { serial, x, y, .. } => {
                assert_eq!((serial, x, y), (7, 100, 200));
            }
            _ => panic!("wrong variant"),
        }
        let ds = DialogState {
            serial: Serial(4),
            width: 320,
            height: 240,
        };
        match ShellEvent::dialog_configure(&ds) {
            ShellEvent::DialogConfigure { serial, width, .. } => {
                assert_eq!((serial, width), (4, 320));
            }
            _ => panic!("wrong variant"),
        }
    }
}
