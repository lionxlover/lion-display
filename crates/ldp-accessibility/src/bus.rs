//! The accessibility event bus — providers, subscribers, ordered
//! delivery with focus attribution.
//!
//! Content-owning clients announce semantics through their
//! [`Provider`] (the `ldp.a11y.a11y_provider` object): speakable
//! announcements, focus moves, caret moves, state changes. Assistive
//! technologies subscribe (the `a11y_control` scope gate) and receive
//! every event **in order** with the announcing provider's focus
//! attribution.
//!
//! Ordering contract (the Phase 16 exit criterion):
//!
//! * Every event carries a global sequence number stamped at push.
//! * Delivery order across *all* providers is exactly global-seq order
//!   (a stable merge of the per-provider FIFOs).
//! * The per-provider subsequence of a delivery stream is exactly the
//!   provider's push order — no reordering, no drops within the queue
//!   bound.
//! * Attribution is push-time: an event reports the focus the provider
//!   reported *when the event was pushed* (a later focus_changed does
//!   not rewrite an earlier announcement's attribution).
//! * Backpressure: each provider queue is bounded; overflow drops the
//!   OLDEST event (assistive tech prefers fresh state) and counts it —
//!   a provider that floods cannot block the bus.

use std::collections::VecDeque;

use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::ids::ObjectId;

/// How urgently a screen reader should speak an announcement
/// (wire: `ldp.a11y.announce_priority`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnnouncePriority {
    /// Speak when the reader is idle.
    Polite,
    /// Speak now, interrupting polite speech.
    Assertive,
    /// Speak now, interrupting everything.
    Urgent,
}

impl AnnouncePriority {
    /// Wire value (`spec/a11y.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Polite => 1,
            Self::Assertive => 2,
            Self::Urgent => 3,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Polite),
            2 => Some(Self::Assertive),
            3 => Some(Self::Urgent),
            _ => None,
        }
    }
}

/// One semantic event pushed by a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    /// Speakable announcement (`announce` request).
    Announce {
        /// The text.
        text: String,
        /// Urgency hint.
        priority: AnnouncePriority,
    },
    /// Logical focus moved (`focus_changed`).
    FocusChanged {
        /// The focused surface.
        surface: ObjectId,
        /// ARIA-style role.
        role: String,
        /// Accessible label.
        label: String,
    },
    /// Text caret moved (`caret_moved`).
    CaretMoved {
        /// Surface the caret is on.
        surface: ObjectId,
        /// Position in surface coordinates.
        x: i32,
        /// Position in surface coordinates.
        y: i32,
    },
    /// Semantic state value changed (`state_changed`).
    StateChanged {
        /// Surface owning the state.
        surface: ObjectId,
        /// State name.
        state: String,
        /// New value.
        value: String,
    },
}

/// The focus attribution attached to a delivery: what the provider had
/// reported as focused at push time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusInfo {
    /// The focused surface.
    pub surface: ObjectId,
    /// Role string.
    pub role: String,
    /// Label string.
    pub label: String,
}

/// One queued push: the event plus its chain coordinates.
#[derive(Clone, Debug)]
struct Queued {
    global_seq: u64,
    event: ProviderEvent,
    focus: Option<FocusInfo>,
}

/// A per-client semantic announcer (`a11y_provider` object state).
#[derive(Debug)]
pub struct Provider {
    key: u32,
    queue: VecDeque<Queued>,
    bound: usize,
    dropped: u64,
    focus: Option<FocusInfo>,
}

impl Provider {
    /// The provider's current focus report.
    #[must_use]
    pub fn focus(&self) -> Option<&FocusInfo> {
        self.focus.as_ref()
    }

    /// Events dropped from this provider's queue (overflow policy).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Events currently queued.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

/// The bus: providers, gated subscribers, the global sequence clock.
#[derive(Debug)]
pub struct A11yBus {
    providers: Vec<Provider>,
    next_seq: u64,
    /// Subscribers that passed the `a11y_control` gate.
    subscribers: Vec<Subscriber>,
}

/// One subscriber's delivered stream (drained by dispatch).
#[derive(Debug)]
struct Subscriber {
    key: u32,
    inbox: VecDeque<Delivery>,
    bound: usize,
}

/// A delivered event: the announcement plus attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// Global sequence (ordering proof).
    pub global_seq: u64,
    /// The delivering provider's key.
    pub provider: u32,
    /// The event.
    pub event: ProviderEvent,
    /// The provider's focus at push time.
    pub focus: Option<FocusInfo>,
}

/// Why a subscription was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubscribeError {
    /// The connection lacks the `a11y_control` scope.
    MissingScope,
    /// Already subscribed.
    AlreadySubscribed,
}

impl A11yBus {
    /// An empty bus.
    #[must_use]
    pub const fn new() -> Self {
        A11yBus {
            providers: Vec::new(),
            next_seq: 1,
            subscribers: Vec::new(),
        }
    }

    /// Create (or return) the client's provider. The queue bound is the
    /// per-provider backpressure limit.
    pub fn provider(&mut self, key: u32, queue_bound: usize) -> &mut Provider {
        if let Some(pos) = self.providers.iter().position(|p| p.key == key) {
            return &mut self.providers[pos];
        }
        self.providers.push(Provider {
            key,
            queue: VecDeque::new(),
            bound: queue_bound.max(1),
            dropped: 0,
            focus: None,
        });
        let last = self.providers.len() - 1;
        &mut self.providers[last]
    }

    /// Push one event for `provider` (the public push path — handles
    /// focus bookkeeping and queueing). Returns the global seq.
    pub fn push(&mut self, key: u32, event: ProviderEvent) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let provider = self.provider(key, 64);
        // Focus bookkeeping: a focus_changed refreshes the attribution.
        if let ProviderEvent::FocusChanged {
            surface,
            role,
            label,
        } = &event
        {
            provider.focus = Some(FocusInfo {
                surface: *surface,
                role: role.clone(),
                label: label.clone(),
            });
        }
        let focus = provider.focus.clone();
        while provider.queue.len() >= provider.bound {
            provider.queue.pop_front();
            provider.dropped += 1;
        }
        provider.queue.push_back(Queued {
            global_seq: seq,
            event,
            focus,
        });
        seq
    }

    /// Subscribe a connection (capability-gated: `a11y_control`).
    ///
    /// # Errors
    ///
    /// [`SubscribeError::MissingScope`] without the scope;
    /// [`SubscribeError::AlreadySubscribed`] on double subscribe.
    pub fn subscribe(
        &mut self,
        key: u32,
        effective: ScopeSet,
        inbox_bound: usize,
    ) -> Result<(), SubscribeError> {
        if !effective.contains(Scope::A11yControl) {
            return Err(SubscribeError::MissingScope);
        }
        if self.subscribers.iter().any(|s| s.key == key) {
            return Err(SubscribeError::AlreadySubscribed);
        }
        self.subscribers.push(Subscriber {
            key,
            inbox: VecDeque::new(),
            bound: inbox_bound.max(1),
        });
        Ok(())
    }

    /// Unsubscribe (connection destroyed or scope revoked).
    pub fn unsubscribe(&mut self, key: u32) {
        self.subscribers.retain(|s| s.key != key);
    }

    /// Dispatch: drain every provider queue in global-seq order into
    /// every subscriber inbox. Returns the number of events fanned out
    /// (providers × subscribers).
    ///
    /// The stable merge: gather all queued events, sort by global seq
    /// (the seqs are unique, so the sort is total), deliver in that
    /// order to each subscriber.
    pub fn dispatch(&mut self) -> u64 {
        let mut batch: Vec<(u32, Queued)> = Vec::new();
        for p in &mut self.providers {
            batch.extend(p.queue.drain(..).map(|q| (p.key, q)));
        }
        batch.sort_by_key(|(_, q)| q.global_seq);
        let mut fanned = 0u64;
        for (provider_key, queued) in batch {
            for s in &mut self.subscribers {
                if s.inbox.len() >= s.bound {
                    s.inbox.pop_front();
                }
                s.inbox.push_back(Delivery {
                    global_seq: queued.global_seq,
                    provider: provider_key,
                    event: queued.event.clone(),
                    focus: queued.focus.clone(),
                });
                fanned += 1;
            }
        }
        fanned
    }

    /// Drain one subscriber's inbox, oldest first.
    #[must_use]
    pub fn drain(&mut self, key: u32) -> Vec<Delivery> {
        match self.subscribers.iter_mut().find(|s| s.key == key) {
            Some(s) => s.inbox.drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// Providers currently registered.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Subscribers currently bound.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Drop a provider (object destroyed): its queued events vanish
    /// with it (they were never delivered; attribution chains stay
    /// consistent for the rest).
    pub fn remove_provider(&mut self, key: u32) {
        self.providers.retain(|p| p.key != key);
    }
}

impl Default for A11yBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: u32) -> ObjectId {
        ObjectId::from_wire(id)
    }

    fn gated_bus() -> A11yBus {
        let mut bus = A11yBus::new();
        bus.subscribe(1, ScopeSet::single(Scope::A11yControl), 64)
            .unwrap();
        bus
    }

    #[test]
    fn scope_gate_is_deny_by_default() {
        let mut bus = A11yBus::new();
        assert_eq!(
            bus.subscribe(1, ScopeSet::NONE, 8).unwrap_err(),
            SubscribeError::MissingScope
        );
        assert_eq!(bus.subscriber_count(), 0);
        assert!(bus
            .subscribe(1, ScopeSet::single(Scope::A11yControl), 8)
            .is_ok());
        assert_eq!(
            bus.subscribe(1, ScopeSet::single(Scope::A11yControl), 8)
                .unwrap_err(),
            SubscribeError::AlreadySubscribed
        );
    }

    #[test]
    fn focus_attribution_is_push_time() {
        let mut bus = gated_bus();
        bus.push(
            10,
            ProviderEvent::FocusChanged {
                surface: surface(0x11),
                role: "button".into(),
                label: "OK".into(),
            },
        );
        bus.push(
            10,
            ProviderEvent::Announce {
                text: "Button OK".into(),
                priority: AnnouncePriority::Polite,
            },
        );
        // A LATER focus change must not rewrite the earlier attribution.
        bus.push(
            10,
            ProviderEvent::FocusChanged {
                surface: surface(0x22),
                role: "text_field".into(),
                label: "Search".into(),
            },
        );
        bus.push(
            10,
            ProviderEvent::Announce {
                text: "Search field".into(),
                priority: AnnouncePriority::Assertive,
            },
        );
        bus.dispatch();
        let deliveries = bus.drain(1);
        assert_eq!(deliveries.len(), 4);
        assert_eq!(deliveries[1].focus.as_ref().unwrap().label, "OK");
        assert_eq!(deliveries[3].focus.as_ref().unwrap().label, "Search");
        // The pre-focus announcement carries no attribution.
        let mut bus2 = gated_bus();
        bus2.push(
            10,
            ProviderEvent::Announce {
                text: "orphan".into(),
                priority: AnnouncePriority::Polite,
            },
        );
        bus2.dispatch();
        assert!(bus2.drain(1)[0].focus.is_none());
    }

    #[test]
    fn provider_queue_overflow_drops_oldest() {
        let mut bus = gated_bus();
        for i in 0..10 {
            bus.provider(10, 4);
            bus.push(
                10,
                ProviderEvent::Announce {
                    text: format!("n{i}"),
                    priority: AnnouncePriority::Polite,
                },
            );
        }
        let p = bus.providers.iter().find(|p| p.key == 10).unwrap();
        assert_eq!(p.dropped(), 6);
        bus.dispatch();
        let deliveries = bus.drain(1);
        assert_eq!(deliveries.len(), 4);
        assert_eq!(
            deliveries[0].event,
            ProviderEvent::Announce {
                text: "n6".into(),
                priority: AnnouncePriority::Polite
            }
        );
    }

    #[test]
    fn empty_dispatch_is_free() {
        let mut bus = gated_bus();
        assert_eq!(bus.dispatch(), 0);
        assert!(bus.drain(1).is_empty());
    }
}
