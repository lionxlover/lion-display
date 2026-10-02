//! Hotplug — connector topology changes.
//!
//! The real source is udev: the kernel emits `change`/`add`/`remove`
//! events on the DRM subsystem whenever a sink appears, disappears, or
//! re-negotiates. [`crate::udev_sys::UdevMonitor`] turns those into
//! [`HotplugSignal`]s; [`HotplugSource`] is the object-safe seam the
//! compositor polls.
//!
//! The policy half lives here: on a signal, the correct response is a
//! *re-probe* of the whole topology (one udev event can mean several
//! connector changes — an MST hub appearing changes its whole tree), and
//! re-probe results are what the registry layer diff-computes into
//! per-connector add/remove events for clients. [`Reprobe`] records a
//! topology diff deterministically for that purpose.
//!
//! The mock source is [`ManualSource`] — tests push signals through the
//! same trait the compositor polls.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use crate::connector::ConnectorStatus;
use crate::ids::ConnectorId;

/// One hotplug notification: an action on a device node.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HotplugSignal {
    /// The action string (`"change"`, `"add"`, `"remove"`).
    pub action: String,
    /// The device node path (`/dev/dri/card0`), if known.
    pub devnode: Option<String>,
}

impl HotplugSignal {
    /// Decode a udev action + devnode pair. Unknown actions are kept
    /// verbatim (future kernel additions must not be dropped on the
    /// floor — a re-probe is always the safe response).
    #[must_use]
    pub fn new(action: &str, devnode: Option<&str>) -> Self {
        Self {
            action: action.to_owned(),
            devnode: devnode.map(str::to_owned),
        }
    }

    /// Whether this signal warrants a re-probe (all of them do).
    #[must_use]
    pub fn needs_reprobe(&self) -> bool {
        true
    }
}

/// The hotplug event source seam.
pub trait HotplugSource {
    /// Drain pending signals (nonblocking).
    fn drain(&mut self) -> Vec<HotplugSignal>;
    /// The pollable file descriptor when the source has one (the udev
    /// monitor socket); `None` for synthetic sources.
    fn fd(&self) -> Option<i32>;
}

/// A manually-driven source for tests and headless composition.
#[derive(Debug, Default)]
pub struct ManualSource {
    queue: VecDeque<HotplugSignal>,
}

impl ManualSource {
    /// An empty source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a signal.
    pub fn push(&mut self, signal: HotplugSignal) {
        self.queue.push_back(signal);
    }
}

impl HotplugSource for ManualSource {
    fn drain(&mut self) -> Vec<HotplugSignal> {
        self.queue.drain(..).collect()
    }

    fn fd(&self) -> Option<i32> {
        None
    }
}

/// A topology diff computed after re-probe, in a deterministic order:
/// removals first (ids released), then additions (ids claimed), then
/// status changes on surviving connectors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reprobe {
    /// Connectors that vanished.
    pub removed: Vec<ConnectorId>,
    /// Connectors that appeared.
    pub added: Vec<ConnectorId>,
    /// Surviving connectors whose status changed, with the new status.
    pub changed: Vec<(ConnectorId, ConnectorStatus)>,
}

impl Reprobe {
    /// Diff two topology snapshots (both in resource-list order).
    ///
    /// # Examples
    /// ```
    /// use ldp_display::hotplug::{Reprobe, HotplugSignal};
    /// use ldp_display::connector::ConnectorStatus;
    /// use ldp_display::ids::ConnectorId;
    ///
    /// let a = vec![(ConnectorId::new(91).unwrap(), ConnectorStatus::Connected)];
    /// let b = vec![
    ///     (ConnectorId::new(91).unwrap(), ConnectorStatus::Disconnected),
    ///     (ConnectorId::new(94).unwrap(), ConnectorStatus::Connected),
    /// ];
    /// let diff = Reprobe::diff(&a, &b);
    /// assert_eq!(diff.removed, Vec::<ConnectorId>::new());
    /// assert_eq!(diff.added, vec![ConnectorId::new(94).unwrap()]);
    /// assert_eq!(diff.changed, vec![(ConnectorId::new(91).unwrap(), ConnectorStatus::Disconnected)]);
    /// ```
    #[must_use]
    pub fn diff(
        before: &[(ConnectorId, ConnectorStatus)],
        after: &[(ConnectorId, ConnectorStatus)],
    ) -> Self {
        let mut removed = Vec::new();
        for (id, _) in before {
            if after.iter().all(|(other, _)| other != id) {
                removed.push(*id);
            }
        }
        let mut added = Vec::new();
        for (id, _) in after {
            if before.iter().all(|(other, _)| other != id) {
                added.push(*id);
            }
        }
        let mut changed = Vec::new();
        for (id, status) in after {
            if let Some((_, old)) = before.iter().find(|(other, _)| other == id) {
                if old != status {
                    changed.push((*id, *status));
                }
            }
        }
        Self {
            removed,
            added,
            changed,
        }
    }
}

/// Snapshot every connector's status in resource-list order — the
/// re-probe's "after" (and the compositor's remembered "before"):
/// feeding two snapshots to [`Reprobe::diff`] yields the deterministic
/// topology change the re-arrangement acts on (Phase 26). Connectors
/// whose snapshot cannot be read are skipped — a connector that stops
/// answering mid-walk is gone for every practical purpose, and the
/// next re-probe diff will say so.
#[must_use]
pub fn topology_statuses(
    backend: &dyn crate::backend::KmsBackend,
) -> Vec<(ConnectorId, ConnectorStatus)> {
    let Ok(top) = backend.topology() else {
        return Vec::new();
    };
    top.connectors
        .iter()
        .filter_map(|c| backend.connector_info(*c).ok().map(|i| (*c, i.status)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_always_reprobe() {
        for action in ["change", "add", "remove", "bind", "future-action"] {
            assert!(HotplugSignal::new(action, Some("/dev/dri/card0")).needs_reprobe());
        }
        let s = HotplugSignal::new("change", None);
        assert_eq!(s.action, "change");
        assert!(s.devnode.is_none());
    }

    #[test]
    fn manual_source_drains_once() {
        let mut src = ManualSource::new();
        src.push(HotplugSignal::new("change", Some("/dev/dri/card0")));
        src.push(HotplugSignal::new("add", Some("/dev/dri/card1")));
        let drained = src.drain();
        assert_eq!(drained.len(), 2);
        assert!(src.drain().is_empty());
        assert!(src.fd().is_none());
    }

    #[test]
    fn diff_orders_removals_then_additions_then_changes() {
        use ConnectorStatus::*;
        let before = vec![
            (ConnectorId::new(91).unwrap(), Connected),
            (ConnectorId::new(92).unwrap(), Disconnected),
            (ConnectorId::new(93).unwrap(), Connected),
        ];
        let after = vec![
            (ConnectorId::new(91).unwrap(), Disconnected),
            (ConnectorId::new(93).unwrap(), Connected),
            (ConnectorId::new(94).unwrap(), Connected),
        ];
        let diff = Reprobe::diff(&before, &after);
        assert_eq!(diff.removed, vec![ConnectorId::new(92).unwrap()]);
        assert_eq!(diff.added, vec![ConnectorId::new(94).unwrap()]);
        assert_eq!(
            diff.changed,
            vec![(ConnectorId::new(91).unwrap(), Disconnected)]
        );
    }

    #[test]
    fn topology_statuses_snapshots_the_mock_in_resource_order() {
        use ConnectorStatus::*;
        let mut dev = crate::MockDevice::laptop_dual();
        let snap = topology_statuses(&dev);
        // Resource order, one entry per connector, statuses as preset.
        let ids: Vec<u32> = snap.iter().map(|(id, _)| id.raw()).collect();
        assert_eq!(ids, vec![91, 92, 93]);
        assert_eq!(
            snap.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            vec![Connected, Disconnected, Connected]
        );
        // After a hotplug unplug, the snapshot changes — the diff says so.
        dev.hotplug_disconnect(ConnectorId::new(91).unwrap());
        let after = topology_statuses(&dev);
        let diff = Reprobe::diff(&snap, &after);
        assert_eq!(
            diff.changed,
            vec![(ConnectorId::new(91).unwrap(), Disconnected)]
        );
        assert!(diff.removed.is_empty() && diff.added.is_empty());
    }
}
