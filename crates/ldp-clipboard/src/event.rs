//! The typed data-event vocabulary and its wire encoding.
//!
//! [`DataEvent`] mirrors `spec/data.toml` event-for-event: one Rust
//! variant per wire event, the same argument order, the same value
//! domains. Encoding goes through the compiled schema registry
//! ([`ldp_protocol::REGISTRY`]) — the variant names its interface and
//! event, the registry supplies the opcode, and [`DataEvent::args`]
//! builds the wire values, so a typo in either place fails loudly at
//! encode time instead of drifting silently. The conformance suites
//! round-trip every emitted message through the codec and the strict
//! signature checker.
//!
//! FD note: `data_source.send` carries an `fd` argument as
//! [`Value::Fd`] — an *index* into the message's ancillary table. The
//! message itself carries only the table size (the transport owns the
//! FDs); the manager emits the index and the integrator attaches the
//! pipe write end as ancillary datum 0.

#![forbid(unsafe_code)]

use crate::dnd::{ActionSet, DndAction};
use crate::offer::OfferKind;
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::{Direction, Message, REGISTRY};

/// Every event the `ldp.data` module defines.
#[derive(Clone, PartialEq, Debug)]
pub enum DataEvent {
    // ----- data_source (to the source client) -----
    /// `data_source.target` — the accepted MIME type (empty: none).
    SourceTarget {
        /// The accepted MIME (empty string: none accepted).
        mime: String,
    },
    /// `data_source.actions` — the currently available action set.
    SourceActions {
        /// Available actions.
        actions: ActionSet,
    },
    /// `data_source.send` — serve `mime` into the pipe.
    SourceSend {
        /// The MIME being served.
        mime: String,
        /// Ancillary FD index (always 0 — one FD per send).
        fd_index: u32,
    },
    /// `data_source.cancelled` — the source is no longer the
    /// selection / the drag aborted.
    SourceCancelled,
    /// `data_source.dnd_drop_performed` — the user dropped.
    SourceDropPerformed,
    /// `data_source.dnd_finished` — release drag-side resources.
    SourceFinished,

    // ----- data_device (to the receiver client) -----
    /// `data_device.data_offer` — a new offer object was created.
    DeviceDataOffer {
        /// The server-chosen object id.
        id: ObjectId,
    },
    /// `data_device.selection` — the clipboard changed.
    DeviceSelection {
        /// The new offer (null: emptied or owner destroyed).
        offer: Option<ObjectId>,
    },
    /// `data_device.primary_selection` — the primary changed.
    DevicePrimarySelection {
        /// The new offer (null: emptied or owner destroyed).
        offer: Option<ObjectId>,
    },
    /// `data_device.enter` — a drag entered a surface.
    DeviceEnter {
        /// The surface entered.
        surface: ObjectId,
        /// Position in surface coordinates.
        pos: PointF,
        /// The drag offer (null when the client has no permission to
        /// see it — denied receivers still get enter/leave so their
        /// UI can react).
        offer: Option<ObjectId>,
    },
    /// `data_device.motion` — drag motion within the surface.
    DeviceMotion {
        /// Position in surface coordinates.
        pos: PointF,
    },
    /// `data_device.drop` — the user dropped here.
    DeviceDrop,
    /// `data_device.leave` — the drag left / was cancelled.
    DeviceLeave,

    // ----- data_offer (to the receiver client) -----
    /// `data_offer.offer` — one offered MIME type.
    OfferOffer {
        /// The offered MIME.
        mime: String,
    },
    /// `data_offer.source_actions` — the source's action set.
    OfferSourceActions {
        /// Actions offered by the drag source.
        actions: ActionSet,
    },
    /// `data_offer.action` — the compositor's pick after negotiation.
    OfferAction {
        /// The negotiated action.
        action: DndAction,
    },
}

impl DataEvent {
    /// Build the offer-list publication events for a fresh offer:
    /// the `data_offer` announcement followed by one `offer` event
    /// per MIME (and `source_actions` for drag offers, before the
    /// enter event that references the offer).
    #[must_use]
    pub fn publication(
        id: ObjectId,
        offers: &[crate::mime::Mime],
        kind: OfferKind,
    ) -> Vec<DataEvent> {
        let mut out = vec![DataEvent::DeviceDataOffer { id }];
        for m in offers {
            out.push(DataEvent::OfferOffer {
                mime: m.to_string(),
            });
        }
        if kind == OfferKind::Drag {
            out.push(DataEvent::OfferSourceActions {
                actions: ActionSet::ALL,
            });
        }
        out
    }

    /// The emitting interface (FQ name).
    #[must_use]
    pub const fn interface(&self) -> &'static str {
        match self {
            DataEvent::SourceTarget { .. }
            | DataEvent::SourceActions { .. }
            | DataEvent::SourceSend { .. }
            | DataEvent::SourceCancelled
            | DataEvent::SourceDropPerformed
            | DataEvent::SourceFinished => "ldp.data.data_source",
            DataEvent::DeviceDataOffer { .. }
            | DataEvent::DeviceSelection { .. }
            | DataEvent::DevicePrimarySelection { .. }
            | DataEvent::DeviceEnter { .. }
            | DataEvent::DeviceMotion { .. }
            | DataEvent::DeviceDrop
            | DataEvent::DeviceLeave => "ldp.data.data_device",
            DataEvent::OfferOffer { .. }
            | DataEvent::OfferSourceActions { .. }
            | DataEvent::OfferAction { .. } => "ldp.data.data_offer",
        }
    }

    /// The event name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            DataEvent::SourceTarget { .. } => "target",
            DataEvent::SourceActions { .. } => "actions",
            DataEvent::SourceSend { .. } => "send",
            DataEvent::SourceCancelled => "cancelled",
            DataEvent::SourceDropPerformed => "dnd_drop_performed",
            DataEvent::SourceFinished => "dnd_finished",
            DataEvent::DeviceDataOffer { .. } => "data_offer",
            DataEvent::DeviceSelection { .. } => "selection",
            DataEvent::DevicePrimarySelection { .. } => "primary_selection",
            DataEvent::DeviceEnter { .. } => "enter",
            DataEvent::DeviceMotion { .. } => "motion",
            DataEvent::DeviceDrop => "drop",
            DataEvent::DeviceLeave => "leave",
            DataEvent::OfferOffer { .. } => "offer",
            DataEvent::OfferSourceActions { .. } => "source_actions",
            DataEvent::OfferAction { .. } => "action",
        }
    }

    /// The wire arguments, in spec order.
    #[must_use]
    pub fn args(&self) -> Vec<Value> {
        fn s(v: &str) -> Value {
            Value::String(v.into())
        }
        fn f(v: f32) -> Value {
            Value::Float32(v)
        }
        fn actions_arg(actions: ActionSet) -> Value {
            Value::Bitset(ldp_core::bitset::Bitset128::from_words([
                actions.to_bits(),
                0,
                0,
                0,
            ]))
        }
        match self {
            DataEvent::SourceTarget { mime } | DataEvent::OfferOffer { mime } => vec![s(mime)],
            DataEvent::SourceActions { actions } | DataEvent::OfferSourceActions { actions } => {
                vec![actions_arg(*actions)]
            }
            DataEvent::SourceSend { mime, fd_index } => vec![s(mime), Value::Fd(*fd_index)],
            DataEvent::SourceCancelled
            | DataEvent::SourceDropPerformed
            | DataEvent::SourceFinished
            | DataEvent::DeviceDrop
            | DataEvent::DeviceLeave => vec![],
            DataEvent::DeviceDataOffer { id } => vec![Value::NewId(*id)],
            DataEvent::DeviceSelection { offer } | DataEvent::DevicePrimarySelection { offer } => {
                vec![Value::Object(*offer)]
            }
            DataEvent::DeviceEnter {
                surface,
                pos,
                offer,
            } => vec![
                Value::Object(Some(*surface)),
                f(pos.x),
                f(pos.y),
                Value::Object(*offer),
            ],
            DataEvent::DeviceMotion { pos } => vec![f(pos.x), f(pos.y)],
            DataEvent::OfferAction { action } => vec![Value::Enum(action.to_wire())],
        }
    }

    /// Build the wire message on `object` (the emitting protocol
    /// object: the source/offer/device the event belongs to).
    ///
    /// # Panics
    ///
    /// When the interface or event is missing from the compiled
    /// schema (a build-time conformance failure, not a runtime
    /// condition).
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

    /// The FD count this event's encoding requires (1 for `send`,
    /// 0 otherwise — the integrator attaches the pipe write end).
    #[must_use]
    pub const fn fd_count(&self) -> u32 {
        match self {
            DataEvent::SourceSend { .. } => 1,
            _ => 0,
        }
    }
}

/// The direction token re-exported for doc cross-references.
#[allow(unused_imports)]
use Direction as _DirectionRef;

#[cfg(test)]
mod tests {
    use super::*;

    const OBJ: ObjectId = ObjectId::from_wire(0x3001);

    #[test]
    fn send_event_declares_one_fd() {
        let e = DataEvent::SourceSend {
            mime: "image/png".into(),
            fd_index: 0,
        };
        assert_eq!(e.fd_count(), 1);
        let m = e.to_message(OBJ);
        assert_eq!(m.required_fd_count(), 1);
        assert_eq!(m.args[1], Value::Fd(0));
    }

    #[test]
    fn bitset_events_word_zero_only() {
        let e = DataEvent::SourceActions {
            actions: ActionSet::build(true, false, true),
        };
        let m = e.to_message(OBJ);
        assert_eq!(
            m.args[0],
            Value::Bitset(ldp_core::bitset::Bitset128::from_words([0b101, 0, 0, 0]))
        );
    }

    #[test]
    fn publication_lists_every_offer() {
        let offers = [
            crate::mime::Mime::parse(
                "text/plain;charset=utf-8",
                &ldp_core::limits::Limits::default(),
            )
            .unwrap(),
            crate::mime::Mime::parse("image/png", &ldp_core::limits::Limits::default()).unwrap(),
        ];
        let pub_events = DataEvent::publication(OBJ, &offers, OfferKind::Selection);
        assert_eq!(pub_events.len(), 3);
        assert!(matches!(pub_events[0], DataEvent::DeviceDataOffer { .. }));
        assert!(matches!(pub_events[1], DataEvent::OfferOffer { .. }));
        assert!(matches!(pub_events[2], DataEvent::OfferOffer { .. }));
        let drag = DataEvent::publication(OBJ, &offers, OfferKind::Drag);
        assert_eq!(drag.len(), 4);
        assert!(matches!(drag[3], DataEvent::OfferSourceActions { .. }));
    }

    #[test]
    fn action_event_uses_wire_enum() {
        let e = DataEvent::OfferAction {
            action: DndAction::Move,
        };
        assert_eq!(e.to_message(OBJ).args[0], Value::Enum(2));
    }
}
