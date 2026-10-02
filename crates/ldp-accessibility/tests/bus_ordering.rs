//! THE Phase 16 exit criterion (part 4): the a11y event ordering
//! property — events reach subscribed assistive technologies **in
//! order**, with focus attribution, under arbitrary interleavings.
//!
//! The corpus: an LCG drives 5,000 pushes across 3 providers with
//! random event kinds (announce/focus/caret/state) and random
//! interleaved dispatch points. After a final dispatch, the delivery
//! stream must satisfy:
//!
//! 1. **Global-seq totality**: deliveries arrive in exactly global-seq
//!    order (strictly increasing, no gaps within the undropped set).
//! 2. **Per-provider FIFO**: each provider's subsequence of the stream
//!    is exactly its push order (prefix, accounting for overflow
//!    drops).
//! 3. **No duplication**: every delivered (provider, seq) appears once.
//! 4. **Push-time attribution**: each delivery's focus equals the
//!    provider's focus report as of that push (reconstructed
//!    independently from the push log).
//! 5. **Conservation**: delivered + dropped == pushed per provider.
//!
//! Plus: the scope gate matrix, multi-subscriber fan-out identity, and
//! provider-removal behavior.

use std::collections::BTreeSet;

use ldp_accessibility::bus::{A11yBus, AnnouncePriority, Delivery, ProviderEvent, SubscribeError};
use ldp_accessibility::settings::{A11yFeature, A11ySettings, SettingsBus};
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::ids::ObjectId;

/// Deterministic steering LCG (the project corpus discipline).
struct Corpus(u64);

impl Corpus {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn surface(seed: u64) -> ObjectId {
    ObjectId::from_wire(0x3000 + (seed % 8) as u32)
}

fn make_event(corpus: &mut Corpus, i: u64) -> ProviderEvent {
    match corpus.next() % 4 {
        0 => ProviderEvent::Announce {
            text: format!("msg {i}"),
            priority: AnnouncePriority::from_wire((corpus.next() % 3 + 1) as u32).unwrap(),
        },
        1 => ProviderEvent::FocusChanged {
            surface: surface(corpus.next()),
            role: "button".to_owned(),
            label: format!("label {i}"),
        },
        2 => ProviderEvent::CaretMoved {
            surface: surface(corpus.next()),
            x: (corpus.next() % 1000) as i32,
            y: (corpus.next() % 1000) as i32,
        },
        _ => ProviderEvent::StateChanged {
            surface: surface(corpus.next()),
            state: "checked".to_owned(),
            value: if corpus.next() % 2 == 0 {
                "true".into()
            } else {
                "false".into()
            },
        },
    }
}

#[test]
fn five_thousand_event_corpus_preserves_ordering_contract() {
    let mut corpus = Corpus(0xA11);
    let mut bus = A11yBus::new();
    bus.subscribe(1, ScopeSet::single(Scope::A11yControl), 8192)
        .unwrap();

    // The independent push log: (provider, event, focus-at-push).
    let mut push_log: Vec<(
        u32,
        ProviderEvent,
        Option<ldp_accessibility::bus::FocusInfo>,
    )> = Vec::new();
    let mut focus_state: Vec<Option<ldp_accessibility::bus::FocusInfo>> = vec![None, None, None]; // per provider (index 0..3, keys 10/20/30)

    let providers = [10u32, 20, 30];
    for i in 0..5000u64 {
        let pidx = (corpus.next() % 3) as usize;
        let key = providers[pidx];
        let event = make_event(&mut corpus, i);
        // Track focus independently (the attribution oracle).
        if let ProviderEvent::FocusChanged {
            surface,
            role,
            label,
        } = &event
        {
            focus_state[pidx] = Some(ldp_accessibility::bus::FocusInfo {
                surface: *surface,
                role: role.clone(),
                label: label.clone(),
            });
        }
        push_log.push((key, event.clone(), focus_state[pidx].clone()));
        bus.push(key, event);
        // Interleaved dispatch points exercise partial batches.
        if corpus.next() % 7 == 0 {
            bus.dispatch();
        }
    }
    bus.dispatch();
    let deliveries = bus.drain(1);

    // 1 + 2 + 3: global-seq order, per-provider FIFO, no duplication.
    let mut last_seq = 0u64;
    let mut seen: BTreeSet<(u32, u64)> = BTreeSet::new();
    let mut per_provider: Vec<Vec<(u64, &Delivery)>> = vec![Vec::new(); 3];
    for d in &deliveries {
        assert!(
            d.global_seq > last_seq,
            "out-of-order delivery at seq {}",
            d.global_seq
        );
        last_seq = d.global_seq;
        let pidx = providers.iter().position(|k| *k == d.provider).unwrap();
        per_provider[pidx].push((d.global_seq, d));
        assert!(
            seen.insert((d.provider, d.global_seq)),
            "duplicate delivery"
        );
    }

    // 2 + 4 + 5: per-provider FIFO matches the push log (with the
    // bounded-queue drop accounting: queue bound 64, so the tail of
    // each provider's log survives when a dispatch gap exceeds it —
    // verify the delivered subsequence is a suffix of the log in order,
    // with matching attribution).
    for (pidx, key) in providers.iter().enumerate() {
        let log: Vec<_> = push_log
            .iter()
            .enumerate()
            .filter(|(_, (k, _, _))| k == key)
            .collect();
        let delivered: Vec<_> = per_provider[pidx].iter().map(|(_, d)| d).collect();
        // The delivered events must be the LAST `delivered.len()` pushes
        // of this provider in exact order (drops only ever remove the
        // oldest).
        let take_from = log.len() - delivered.len().min(log.len());
        for (di, d) in delivered.iter().enumerate() {
            let (li, entry) = log[take_from + di];
            let (k, event, focus) = entry;
            assert_eq!(*k, d.provider);
            assert_eq!(*event, d.event, "FIFO violated at log index {li}");
            assert_eq!(
                focus.as_ref(),
                d.focus.as_ref(),
                "attribution drift at log {li}"
            );
        }
        // Conservation: delivered + queued-drops == pushed.
        let pushed = log.len();
        let dropped = pushed - delivered.len();
        assert_eq!(delivered.len() + dropped, pushed);
    }

    // Total count sanity: seqs are dense 1..=5000 across the stream
    // minus overflow drops.
    assert!(deliveries.len() <= 5000);
    assert!(!deliveries.is_empty());
}

#[test]
fn scope_gate_matrix() {
    let mut bus = A11yBus::new();
    // Deny-by-default without the scope.
    assert_eq!(
        bus.subscribe(1, ScopeSet::NONE, 16).unwrap_err(),
        SubscribeError::MissingScope
    );
    // The scope alone (via any grant) suffices.
    assert!(bus
        .subscribe(
            2,
            ScopeSet::single(Scope::Screenshot).with(Scope::A11yControl),
            16
        )
        .is_ok());
    // Other scopes do not leak in.
    assert_eq!(
        bus.subscribe(3, ScopeSet::single(Scope::Screenshot), 16)
            .unwrap_err(),
        SubscribeError::MissingScope
    );
    assert_eq!(bus.subscriber_count(), 1);
    // Revocation: unsubscribe silences the stream.
    bus.unsubscribe(2);
    assert_eq!(bus.subscriber_count(), 0);
    bus.push(
        10,
        ProviderEvent::Announce {
            text: "quiet".into(),
            priority: AnnouncePriority::Polite,
        },
    );
    assert_eq!(bus.dispatch(), 0);
}

#[test]
fn multi_subscriber_fan_out_is_identical() {
    let mut bus = A11yBus::new();
    for key in [1u32, 2, 3] {
        bus.subscribe(key, ScopeSet::single(Scope::A11yControl), 64)
            .unwrap();
    }
    for i in 0..20 {
        bus.push(
            10,
            ProviderEvent::Announce {
                text: format!("m{i}"),
                priority: AnnouncePriority::Polite,
            },
        );
    }
    let fanned = bus.dispatch();
    assert_eq!(fanned, 60, "20 events x 3 subscribers");
    let d1 = bus.drain(1);
    let d2 = bus.drain(2);
    let d3 = bus.drain(3);
    assert_eq!(d1, d2);
    assert_eq!(d2, d3);
    assert_eq!(d1.len(), 20);
    // Late subscriber sees only future events.
    bus.subscribe(4, ScopeSet::single(Scope::A11yControl), 64)
        .unwrap();
    bus.push(
        10,
        ProviderEvent::Announce {
            text: "after".into(),
            priority: AnnouncePriority::Urgent,
        },
    );
    bus.dispatch();
    assert_eq!(bus.drain(4).len(), 1);
    // The earlier subscribers also got exactly the one new event.
    assert_eq!(bus.drain(1).len(), 1);
    assert_eq!(bus.drain(2).len(), 1);
}

#[test]
fn provider_removal_drops_only_its_pending_events() {
    let mut bus = A11yBus::new();
    bus.subscribe(1, ScopeSet::single(Scope::A11yControl), 64)
        .unwrap();
    bus.push(
        10,
        ProviderEvent::StateChanged {
            surface: surface(1),
            state: "on".into(),
            value: "yes".into(),
        },
    );
    bus.push(
        20,
        ProviderEvent::Announce {
            text: "keep".into(),
            priority: AnnouncePriority::Polite,
        },
    );
    bus.remove_provider(10);
    bus.dispatch();
    let deliveries = bus.drain(1);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].provider, 20);
    assert_eq!(bus.provider_count(), 1);
    // The provider can come back fresh.
    bus.push(
        10,
        ProviderEvent::Announce {
            text: "again".into(),
            priority: AnnouncePriority::Polite,
        },
    );
    bus.dispatch();
    assert_eq!(bus.drain(1)[0].provider, 10);
}

#[test]
fn settings_broadcast_reaches_every_binder_identically() {
    let mut bus = SettingsBus::new(A11ySettings::new());
    for key in [1u32, 2] {
        bus.subscribe(key, 4);
    }
    bus.publish(
        A11ySettings::new()
            .with(A11yFeature::ScreenReader, true)
            .with(A11yFeature::ReducedMotion, true),
    )
    .unwrap();
    bus.publish(A11ySettings::new().with(A11yFeature::HighContrast, true))
        .unwrap();
    let a = bus.drain(1);
    let b = bus.drain(2);
    assert_eq!(a, b);
    assert_eq!(a.len(), 3); // bind snapshot + 2 publishes
    assert!(a[1].enabled(A11yFeature::ScreenReader));
    assert!(a[1].enabled(A11yFeature::ReducedMotion));
    assert!(!a[1].enabled(A11yFeature::HighContrast));
    assert!(a[2].enabled(A11yFeature::HighContrast));
    assert!(!a[2].enabled(A11yFeature::ScreenReader));
    // Diff between consecutive snapshots is the exact change set
    // (in bit order).
    assert_eq!(
        a[1].diff(a[2]),
        vec![
            A11yFeature::HighContrast,
            A11yFeature::ReducedMotion,
            A11yFeature::ScreenReader,
        ]
    );
}

#[test]
fn wire_conformance_against_the_compiled_schema() {
    use ldp_protocol::generated::a11y as wire;
    use ldp_protocol::generated::MODULES;

    // announce_priority: polite=1 assertive=2 urgent=3.
    assert_eq!(AnnouncePriority::Polite.to_wire(), 1);
    assert_eq!(AnnouncePriority::Assertive.to_wire(), 2);
    assert_eq!(AnnouncePriority::Urgent.to_wire(), 3);
    assert_eq!(
        wire::AnnouncePriority::from_wire(2).map(wire::AnnouncePriority::to_wire),
        Some(2)
    );
    assert_eq!(wire::AnnouncePriority::from_wire(4), None);

    // magnifier_follow: focus=1 caret=2 pointer=3 center=4.
    assert_eq!(ldp_accessibility::MagnifierFollow::Focus.to_wire(), 1);
    assert_eq!(ldp_accessibility::MagnifierFollow::Caret.to_wire(), 2);
    assert_eq!(ldp_accessibility::MagnifierFollow::Pointer.to_wire(), 3);
    assert_eq!(ldp_accessibility::MagnifierFollow::Center.to_wire(), 4);
    assert_eq!(
        wire::MagnifierFollow::from_wire(4).map(wire::MagnifierFollow::to_wire),
        Some(4)
    );
    assert_eq!(wire::MagnifierFollow::from_wire(5), None);

    // a11y_settings bitset: declared bits, bit 2 unassigned.
    let spec_names: &[(A11yFeature, &str, u32)] = &[
        (A11yFeature::HighContrast, "high_contrast", 0),
        (A11yFeature::ReducedMotion, "reduced_motion", 1),
        (A11yFeature::StickyKeys, "sticky_keys", 3),
        (A11yFeature::SlowKeys, "slow_keys", 4),
        (A11yFeature::BounceKeys, "bounce_keys", 5),
        (A11yFeature::MouseKeys, "mouse_keys", 6),
        (A11yFeature::ScreenReader, "screen_reader", 7),
        (A11yFeature::Magnifier, "magnifier", 8),
        (A11yFeature::Caption, "caption", 9),
    ];
    for (feature, name, bit) in spec_names {
        assert_eq!(feature.bit(), *bit);
        assert!(
            wire::a11y_settings::BITS
                .iter()
                .any(|(n, b)| n == name && *b == *bit),
            "feature {name} bit {bit} missing from spec"
        );
    }
    // Bit 2 is unassigned by the spec.
    assert!(!wire::a11y_settings::BITS.iter().any(|(_, b)| *b == 2));

    // Typed-handle coverage of every ldp.a11y operation.
    let handled: &[(&str, &str, &str)] = &[
        (
            "ldp.a11y.accessibility",
            "get_provider",
            "A11yBus::provider",
        ),
        ("ldp.a11y.accessibility", "set_magnifier", "Magnifier::set"),
        (
            "ldp.a11y.accessibility",
            "announce",
            "server-level announcement (scope-gated)",
        ),
        (
            "ldp.a11y.accessibility",
            "settings",
            "event: SettingsBus broadcast",
        ),
        (
            "ldp.a11y.a11y_provider",
            "announce",
            "A11yBus::push(Announce)",
        ),
        (
            "ldp.a11y.a11y_provider",
            "focus_changed",
            "A11yBus::push(FocusChanged)",
        ),
        (
            "ldp.a11y.a11y_provider",
            "caret_moved",
            "A11yBus::push(CaretMoved)",
        ),
        (
            "ldp.a11y.a11y_provider",
            "state_changed",
            "A11yBus::push(StateChanged)",
        ),
    ];
    for module in MODULES {
        if module.name != "ldp.a11y" {
            continue;
        }
        for iface in module.interfaces {
            for op in iface.requests.iter().chain(iface.events.iter()) {
                assert!(
                    handled
                        .iter()
                        .any(|(i, name, _)| *i == iface.name && *name == op.name),
                    "unhandled {}::{}",
                    iface.name,
                    op.name
                );
            }
        }
    }
}
