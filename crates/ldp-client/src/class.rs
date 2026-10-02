//! Event classification into dispatch lanes.
//!
//! `docs/architecture.md` §5: the client library keeps **distinct queue
//! lanes per event class** so a burst of `frame_target` events can
//! never delay pointer motion beyond one dispatch cycle. The class of
//! an event is a property of its *interface and operation name*, fixed
//! by the protocol's v1 surface; [`class_of`] encodes that surface as
//! one rule per module plus a short exception list.
//!
//! Classes and their dispatch discipline (see `queue.rs`):
//!
//! | Class | Discipline |
//! |---|---|
//! | [`EventClass::Input`] | highest priority, FIFO, never starved by any other lane |
//! | [`EventClass::Data`] | second priority, FIFO (offers are small and rare) |
//! | [`EventClass::Control`] | barrier events: dispatched only once every earlier-arrived event has been dispatched (strict wire order w.r.t. the past) |
//! | [`EventClass::Configuration`] | FIFO, may overtake presentation-class events |
//! | [`EventClass::Presentation`] | lowest priority, FIFO |
//!
//! Unknown interfaces (a future module this build has no schema for)
//! classify as [`EventClass::Control`] — the maximally conservative choice: a
//! barrier never reorders against anything that arrived before it, so
//! unclassifiable traffic cannot break ordering invariants it might
//! secretly depend on.

/// Which dispatch lane an event belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum EventClass {
    /// Input-class events (pointer motion, keys, touch): highest
    /// priority, never starved.
    Input,
    /// Data-transfer events (clipboard/DnD offers): never dropped,
    /// dispatched before control polls starve them.
    Data,
    /// Lifecycle and round-trip events (`connection.*`, `registry.*`):
    /// barriers with strict order against the past.
    Control,
    /// Configuration/state events (output geometry, shell configure):
    /// FIFO, above presentation.
    Configuration,
    /// Frame-cadence events (`frame_target`, `presented`, buffer
    /// release): lowest priority, never ahead of input.
    Presentation,
}

impl EventClass {
    /// The lane's dispatch priority (lower = earlier). Input first,
    /// presentation last; control's effective position is modulated by
    /// its barrier rule (see [`crate::queue`]).
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            EventClass::Input => 0,
            EventClass::Data => 1,
            EventClass::Control => 2,
            EventClass::Configuration => 3,
            EventClass::Presentation => 4,
        }
    }

    /// Whether events of this class are barriers: dispatched only after
    /// every event that arrived before them, whatever lane it sits in.
    #[must_use]
    pub const fn is_barrier(self) -> bool {
        matches!(self, EventClass::Control)
    }

    /// Stable lane name (diagnostics, test output).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EventClass::Input => "input",
            EventClass::Data => "data",
            EventClass::Control => "control",
            EventClass::Configuration => "configuration",
            EventClass::Presentation => "presentation",
        }
    }
}

/// The input-module events that are *not* input-class: they configure
/// devices rather than report input (matching the architecture's split
/// between device configuration and input streams).
const INPUT_CONFIGURATION_EVENTS: &[(&str, &str)] = &[
    ("ldp.input.seat", "capabilities"),
    ("ldp.input.seat", "name"),
    ("ldp.input.keyboard", "keymap"),
    ("ldp.input.keyboard", "repeat_info"),
];

/// Presentation-class events outside the input/data modules.
const PRESENTATION_EVENTS: &[(&str, &str)] = &[
    ("ldp.core.surface", "frame_target"),
    ("ldp.core.surface", "presented"),
    ("ldp.core.surface", "frame_dropped"),
    ("ldp.core.buffer", "release"),
];

/// Classify one event by its interface and operation names.
///
/// Rules, in order:
///
/// 1. `ldp.core.connection` and `ldp.core.registry` events are
///    [`EventClass::Control`] — handshakes, round-trip completions, lifecycle, and
///    introspection replies must not reorder against anything.
/// 2. `ldp.input` module events are [`EventClass::Input`], except the device
///    configuration events listed above ([`EventClass::Configuration`]).
/// 3. `ldp.data` module events are [`EventClass::Data`].
/// 4. The presentation set above is [`EventClass::Presentation`]; buffer `release`
///    is frame-cadence traffic by the same reasoning (it gates the
///    client's next frame, so floods of it must not starve input).
/// 5. Every other event of a *known* interface (shell, color, session,
///    security, a11y, and the remaining core surface/output events) is
///    [`EventClass::Configuration`].
/// 6. Events of interfaces absent from the compiled schema are
///    [`EventClass::Control`] (conservative default; see the module docs).
#[must_use]
pub fn class_of(interface: &str, event: &str) -> EventClass {
    // Rule 1: the bootstrap interfaces are control by definition.
    if interface == "ldp.core.connection" || interface == "ldp.core.registry" {
        return EventClass::Control;
    }
    // Rules 2–4: module-scoped defaults with exception lists.
    if let Some(module) = module_of(interface) {
        match module {
            "ldp.input" => {
                if INPUT_CONFIGURATION_EVENTS.contains(&(interface, event)) {
                    return EventClass::Configuration;
                }
                return EventClass::Input;
            }
            "ldp.data" => return EventClass::Data,
            "ldp.core" => {
                if PRESENTATION_EVENTS.contains(&(interface, event)) {
                    return EventClass::Presentation;
                }
                return EventClass::Configuration;
            }
            _ => {}
        }
    }
    // Rule 5 vs 6: known interface -> configuration; unknown -> control.
    if ldp_protocol::REGISTRY.interface(interface).is_some() {
        EventClass::Configuration
    } else {
        EventClass::Control
    }
}

/// The `ldp.<module>` prefix of a fully qualified interface name
/// (None when the name is not qualified — not a v1 interface shape).
fn module_of(interface: &str) -> Option<&str> {
    let idx = interface.rfind('.')?;
    Some(&interface[..idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_protocol::REGISTRY;

    #[test]
    fn bootstrap_interfaces_are_control() {
        for (iface, ev) in [
            ("ldp.core.connection", "welcome"),
            ("ldp.core.connection", "sync_done"),
            ("ldp.core.connection", "error"),
            ("ldp.core.registry", "bound"),
            ("ldp.core.registry", "global"),
            ("ldp.core.registry", "schema"),
        ] {
            assert_eq!(class_of(iface, ev), EventClass::Control, "{iface}.{ev}");
        }
    }

    #[test]
    fn input_events_are_input_class() {
        for (iface, ev) in [
            ("ldp.input.pointer", "motion"),
            ("ldp.input.pointer", "button"),
            ("ldp.input.keyboard", "key"),
            ("ldp.input.touch", "down"),
            ("ldp.input.gestures", "swipe_begin"),
        ] {
            assert_eq!(class_of(iface, ev), EventClass::Input, "{iface}.{ev}");
        }
    }

    #[test]
    fn input_configuration_exceptions_hold() {
        for (iface, ev) in INPUT_CONFIGURATION_EVENTS {
            assert_eq!(
                class_of(iface, ev),
                EventClass::Configuration,
                "{iface}.{ev}"
            );
        }
    }

    #[test]
    fn presentation_and_config_split() {
        assert_eq!(
            class_of("ldp.core.surface", "frame_target"),
            EventClass::Presentation
        );
        assert_eq!(
            class_of("ldp.core.surface", "presented"),
            EventClass::Presentation
        );
        assert_eq!(
            class_of("ldp.core.buffer", "release"),
            EventClass::Presentation
        );
        assert_eq!(
            class_of("ldp.core.surface", "preferred_scale"),
            EventClass::Configuration
        );
        assert_eq!(
            class_of("ldp.core.output", "geometry"),
            EventClass::Configuration
        );
        assert_eq!(
            class_of("ldp.shell.toplevel", "configure"),
            EventClass::Configuration
        );
    }

    #[test]
    fn data_module_is_data_class() {
        assert_eq!(
            class_of("ldp.data.data_device", "selection"),
            EventClass::Data
        );
        assert_eq!(class_of("ldp.data.data_offer", "offer"), EventClass::Data);
    }

    #[test]
    fn unknown_interfaces_default_to_control() {
        assert_eq!(class_of("ldp.future.widget", "ping"), EventClass::Control);
        assert_eq!(
            class_of("not-an-interface", "anything"),
            EventClass::Control
        );
    }

    /// Every event of every compiled interface classifies without
    /// panicking, and the interesting ones land where the architecture
    /// requires (the coverage guarantee behind the lane scheduler).
    #[test]
    fn every_schema_event_classifies() {
        let mut count = 0usize;
        let mut input = 0usize;
        let mut presentation = 0usize;
        let mut control = 0usize;
        for module in REGISTRY.modules() {
            for iface in module.interfaces {
                for op in iface.events {
                    let class = class_of(iface.name, op.name);
                    assert_ne!(class.as_str(), "");
                    match class {
                        EventClass::Input => input += 1,
                        EventClass::Presentation => presentation += 1,
                        EventClass::Control => control += 1,
                        _ => {}
                    }
                    count += 1;
                }
            }
        }
        assert!(
            count >= 100,
            "expected the full v1 event surface, got {count}"
        );
        assert!(input >= 25, "input-class surface shrank: {input}");
        assert!(
            presentation >= 3,
            "presentation-class surface shrank: {presentation}"
        );
        assert!(control >= 10, "control-class surface shrank: {control}");
    }
}
