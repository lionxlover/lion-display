//! Accessibility settings — the broadcast model behind the
//! `ldp.a11y.accessibility.settings` event.
//!
//! Settings are host state (the settings daemon writes them, the
//! compositor broadcasts): the feature-toggle bitset plus the two
//! parameterized values (text scale in Q8, cursor size in logical
//! pixels). Every bound client receives the *same* snapshot on change,
//! so toolkits restyle immediately and deterministically.
//!
//! Wire mapping (`spec/a11y.toml` `a11y_settings` bitset — note bit 2 is
//! deliberately unassigned by the spec):
//!
//! | bit | feature |
//! |-----|---------|
//! | 0 | high contrast |
//! | 1 | reduced motion |
//! | 3 | sticky keys |
//! | 4 | slow keys |
//! | 5 | bounce keys |
//! | 6 | mouse keys |
//! | 7 | screen reader |
//! | 8 | magnifier |
//! | 9 | caption |

/// One accessibility feature toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum A11yFeature {
    /// High-contrast palette.
    HighContrast,
    /// Reduce animation and motion.
    ReducedMotion,
    /// Sticky modifier keys (keyboard access).
    StickyKeys,
    /// Slow keys — hold to accept (keyboard access).
    SlowKeys,
    /// Bounce keys — debounce repeats (keyboard access).
    BounceKeys,
    /// Mouse keys — pointer via keypad (keyboard access).
    MouseKeys,
    /// Screen reader active.
    ScreenReader,
    /// Magnifier active.
    Magnifier,
    /// Live captions active.
    Caption,
}

impl A11yFeature {
    /// Bit index in the `a11y_settings` bitset (wire: bit 2 unassigned).
    #[must_use]
    pub const fn bit(self) -> u32 {
        match self {
            Self::HighContrast => 0,
            Self::ReducedMotion => 1,
            Self::StickyKeys => 3,
            Self::SlowKeys => 4,
            Self::BounceKeys => 5,
            Self::MouseKeys => 6,
            Self::ScreenReader => 7,
            Self::Magnifier => 8,
            Self::Caption => 9,
        }
    }

    /// All features, bit order.
    pub const ALL: [A11yFeature; 9] = [
        Self::HighContrast,
        Self::ReducedMotion,
        Self::StickyKeys,
        Self::SlowKeys,
        Self::BounceKeys,
        Self::MouseKeys,
        Self::ScreenReader,
        Self::Magnifier,
        Self::Caption,
    ];
}

/// The full settings snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11ySettings {
    bits: u128,
    /// Text scale, Q8 (256 = 1.0x). Valid range 256..=1024.
    text_scale_q8: u32,
    /// Cursor size in logical pixels. Valid range 1..=256.
    cursor_size: u32,
}

/// Settings validation failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingsError {
    /// Text scale outside 256..=1024 (1.0x..4.0x).
    TextScaleRange,
    /// Cursor size outside 1..=256.
    CursorSizeRange,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextScaleRange => f.write_str("text scale out of range"),
            Self::CursorSizeRange => f.write_str("cursor size out of range"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl Default for A11ySettings {
    fn default() -> Self {
        // All features off, 1.0x text, 24 px cursor — the platform
        // default a fresh session starts from.
        A11ySettings {
            bits: 0,
            text_scale_q8: 256,
            cursor_size: 24,
        }
    }
}

impl A11ySettings {
    /// The default snapshot (all off, 1.0x, 24 px).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from raw parts, validating the parameter ranges.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] when a parameterized value is out of range.
    pub fn from_raw(
        enabled: &[A11yFeature],
        text_scale_q8: u32,
        cursor_size: u32,
    ) -> Result<Self, SettingsError> {
        if !(256..=1024).contains(&text_scale_q8) {
            return Err(SettingsError::TextScaleRange);
        }
        if !(1..=256).contains(&cursor_size) {
            return Err(SettingsError::CursorSizeRange);
        }
        let mut bits = 0u128;
        for f in enabled {
            bits |= 1u128 << f.bit();
        }
        Ok(A11ySettings {
            bits,
            text_scale_q8,
            cursor_size,
        })
    }

    /// Whether a feature is on.
    #[must_use]
    pub fn enabled(self, feature: A11yFeature) -> bool {
        self.bits & (1u128 << feature.bit()) != 0
    }

    /// A copy with one feature flipped on or off.
    #[must_use]
    pub fn with(self, feature: A11yFeature, on: bool) -> Self {
        let mut bits = self.bits;
        if on {
            bits |= 1u128 << feature.bit();
        } else {
            bits &= !(1u128 << feature.bit());
        }
        Self { bits, ..self }
    }

    /// Text scale (Q8).
    #[must_use]
    pub const fn text_scale_q8(self) -> u32 {
        self.text_scale_q8
    }

    /// Cursor size (logical px).
    #[must_use]
    pub const fn cursor_size(self) -> u32 {
        self.cursor_size
    }

    /// The `settings` event's `enabled` word (bitset bits as declared).
    #[must_use]
    pub const fn enabled_bits(self) -> u128 {
        self.bits
    }

    /// Which features differ between two snapshots (the broadcast's
    /// change set).
    #[must_use]
    pub fn diff(self, other: Self) -> Vec<A11yFeature> {
        let changed = self.bits ^ other.bits;
        A11yFeature::ALL
            .iter()
            .copied()
            .filter(|f| changed & (1u128 << f.bit()) != 0)
            .collect()
    }
}

/// The settings broadcast: one writer, every bound client gets the same
/// snapshot queue.
///
/// Subscribers are per-connection queues (bounded); the compositor's
/// dispatch drains them into `settings` events. A late binder gets the
/// *current* snapshot immediately (the spec's "broadcast to every bound
/// client" — binding is the join point).
#[derive(Debug)]
pub struct SettingsBus {
    current: A11ySettings,
    subscribers: Vec<SettingsQueue>,
}

/// One subscriber's bounded queue of snapshots.
#[derive(Debug)]
struct SettingsQueue {
    key: u32,
    pending: std::collections::VecDeque<A11ySettings>,
    bound: usize,
}

impl SettingsBus {
    /// A bus starting from `initial`.
    #[must_use]
    pub fn new(initial: A11ySettings) -> Self {
        SettingsBus {
            current: initial,
            subscribers: Vec::new(),
        }
    }

    /// The current snapshot.
    #[must_use]
    pub fn current(&self) -> A11ySettings {
        self.current
    }

    /// Publish a new snapshot (validated first): every subscriber's
    /// queue gains it; the writer is rate-limited by queue bound with
    /// coalescing — if a queue is full, undelivered snapshots collapse
    /// to the newest (settings are state, not events; only the latest
    /// matters).
    ///
    /// # Errors
    ///
    /// [`SettingsError`] when the new snapshot is invalid (unchanged bus).
    pub fn publish(&mut self, next: A11ySettings) -> Result<(), SettingsError> {
        // Range-validate through round-trip: parameters are private, so
        // rebuild via the public constructor.
        let rebuilt = A11ySettings::from_raw(
            &A11yFeature::ALL
                .iter()
                .copied()
                .filter(|f| next.enabled(*f))
                .collect::<Vec<_>>(),
            next.text_scale_q8,
            next.cursor_size,
        )?;
        self.current = rebuilt;
        for q in &mut self.subscribers {
            if q.pending.len() >= q.bound {
                q.pending.pop_front();
            }
            q.pending.push_back(rebuilt);
        }
        Ok(())
    }

    /// Bind a subscriber (returns its key). A late binder immediately
    /// sees the current snapshot.
    pub fn subscribe(&mut self, key: u32, queue_bound: usize) {
        let bound = queue_bound.max(1);
        let mut q = SettingsQueue {
            key,
            pending: std::collections::VecDeque::new(),
            bound,
        };
        q.pending.push_back(self.current);
        self.subscribers.push(q);
    }

    /// Unbind (connection destroyed).
    pub fn unsubscribe(&mut self, key: u32) {
        self.subscribers.retain(|q| q.key != key);
    }

    /// Drain one subscriber's pending snapshots, oldest first.
    #[must_use]
    pub fn drain(&mut self, key: u32) -> Vec<A11ySettings> {
        match self.subscribers.iter_mut().find(|q| q.key == key) {
            Some(q) => q.pending.drain(..).collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::bitset::Bitset128;

    #[test]
    fn wire_bits_match_the_spec_layout() {
        // Bit 2 is unassigned by the spec; the declared bits are exact.
        for (feature, bit) in [
            (A11yFeature::HighContrast, 0),
            (A11yFeature::ReducedMotion, 1),
            (A11yFeature::StickyKeys, 3),
            (A11yFeature::SlowKeys, 4),
            (A11yFeature::BounceKeys, 5),
            (A11yFeature::MouseKeys, 6),
            (A11yFeature::ScreenReader, 7),
            (A11yFeature::Magnifier, 8),
            (A11yFeature::Caption, 9),
        ] {
            assert_eq!(feature.bit(), bit);
        }
    }

    #[test]
    fn ranges_are_enforced() {
        assert!(A11ySettings::from_raw(&[], 256, 24).is_ok());
        assert!(A11ySettings::from_raw(&[], 1024, 256).is_ok());
        assert_eq!(
            A11ySettings::from_raw(&[], 255, 24).unwrap_err(),
            SettingsError::TextScaleRange
        );
        assert_eq!(
            A11ySettings::from_raw(&[], 1025, 24).unwrap_err(),
            SettingsError::TextScaleRange
        );
        assert_eq!(
            A11ySettings::from_raw(&[], 256, 0).unwrap_err(),
            SettingsError::CursorSizeRange
        );
        assert_eq!(
            A11ySettings::from_raw(&[], 256, 257).unwrap_err(),
            SettingsError::CursorSizeRange
        );
    }

    #[test]
    fn diff_reports_changed_features_only() {
        let a = A11ySettings::new().with(A11yFeature::HighContrast, true);
        let b = a
            .with(A11yFeature::HighContrast, false)
            .with(A11yFeature::ScreenReader, true);
        let diff = a.diff(b);
        assert_eq!(diff.len(), 2);
        assert!(diff.contains(&A11yFeature::HighContrast));
        assert!(diff.contains(&A11yFeature::ScreenReader));
    }

    #[test]
    fn broadcast_coalesces_under_bound() {
        let mut bus = SettingsBus::new(A11ySettings::new());
        bus.subscribe(1, 2);
        bus.subscribe(2, 8);
        for i in 0..5 {
            bus.publish(
                A11ySettings::new()
                    .with(A11yFeature::ScreenReader, true)
                    .with(A11yFeature::Caption, i % 2 == 0),
            )
            .unwrap();
        }
        // Queue of 2: the two newest snapshots (coalesced; only the
        // latest matters, so the head is the second-newest).
        let q1 = bus.drain(1);
        assert_eq!(q1.len(), 2);
        assert_ne!(q1[0], bus.current());
        assert_eq!(q1[1], bus.current());
        // Queue of 8 holds the bind snapshot plus every publish.
        let q2 = bus.drain(2);
        assert_eq!(q2.len(), 6);
        assert_eq!(q2[0], A11ySettings::new());
        assert_eq!(q2[5], bus.current());
        // Drained queues deliver nothing on re-drain.
        assert!(bus.drain(1).is_empty());
    }

    #[test]
    fn bitset128_interop() {
        // The bits render through the core Bitset128 without surprises.
        let s = A11ySettings::new()
            .with(A11yFeature::Magnifier, true)
            .with(A11yFeature::Caption, true);
        let word = Bitset128::from_words([s.enabled_bits() as u32, 0, 0, 0]);
        assert!(word.test(8));
        assert!(word.test(9));
        assert!(!word.test(7));
    }
}
