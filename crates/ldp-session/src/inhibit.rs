//! Inhibitor cookies — the `ldp.session.session.inhibit` model.
//!
//! An inhibitor is a cookie bound to a connection: it delays the
//! behaviors in its [`InhibitBits`] until released (the cookie's
//! object destroyed, or the connection dropped — logind's
//! disconnect-releases semantics). The effective mask is the union of
//! every live cookie; the idle machine and the manual-sleep gate
//! consume it.
//!
//! `InhibitBits` mirrors `ldp.session.inhibit_bits` (blur/display/
//! idle/suspend at bits 0-3); the ldp-power crate's mask is the same
//! wire contract (cross-checked in tests/lock_vt.rs).

use ldp_core::bitset::Bitset128;

/// Which behaviors a cookie delays (wire: `ldp.session.inhibit_bits`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InhibitBits {
    bits: u32,
}

impl InhibitBits {
    /// Nothing inhibited.
    pub const NONE: InhibitBits = InhibitBits { bits: 0 };
    /// Delay lock-screen dimming/blur (bit 0).
    pub const BLUR: InhibitBits = InhibitBits { bits: 1 << 0 };
    /// Delay display DPMS-off (bit 1).
    pub const DISPLAY: InhibitBits = InhibitBits { bits: 1 << 1 };
    /// Delay idle suspend (bit 2).
    pub const IDLE: InhibitBits = InhibitBits { bits: 1 << 2 };
    /// Delay system suspend (bit 3).
    pub const SUSPEND: InhibitBits = InhibitBits { bits: 1 << 3 };

    /// Union.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        InhibitBits {
            bits: self.bits | other.bits,
        }
    }

    /// Whether all of `other`'s bits are held.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    /// The wire word.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        self.bits
    }

    /// Parse a wire word (unknown bits preserved).
    #[must_use]
    pub const fn from_wire(bits: u32) -> Self {
        InhibitBits { bits }
    }

    fn from_bitset(b: Bitset128) -> Self {
        InhibitBits {
            bits: b.to_words()[0],
        }
    }
}

/// The registry of live cookies.
#[derive(Debug)]
pub struct InhibitRegistry {
    next_cookie: u32,
    /// cookie -> (client, bits)
    held: std::collections::BTreeMap<u32, (u32, InhibitBits)>,
}

impl Default for InhibitRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InhibitRegistry {
    /// An empty registry; cookies start at 1 (0 is the wire's
    /// "inhibited" event payload and must stay distinguishable).
    #[must_use]
    pub const fn new() -> Self {
        InhibitRegistry {
            next_cookie: 1,
            held: std::collections::BTreeMap::new(),
        }
    }

    /// Take an inhibitor for `client`; returns the cookie (the
    /// `inhibited` event's payload).
    pub fn take(&mut self, client: u32, bits: InhibitBits) -> u32 {
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        self.held.insert(cookie, (client, bits));
        cookie
    }

    /// Release one cookie (connection.destroy of the inhibitor
    /// object). Returns the bits it held, if it was live.
    pub fn release(&mut self, cookie: u32) -> Option<InhibitBits> {
        self.held.remove(&cookie).map(|(_, bits)| bits)
    }

    /// A connection went away: release every cookie it held; returns
    /// the union of what was released.
    pub fn client_disconnected(&mut self, client: u32) -> InhibitBits {
        let released: Vec<u32> = self
            .held
            .iter()
            .filter(|(_, (c, _))| *c == client)
            .map(|(cookie, _)| *cookie)
            .collect();
        let mut bits = InhibitBits::NONE;
        for cookie in released {
            if let Some((_, b)) = self.held.remove(&cookie) {
                bits = bits.union(b);
            }
        }
        bits
    }

    /// The union of every live cookie's bits.
    #[must_use]
    pub fn effective(&self) -> InhibitBits {
        self.held
            .values()
            .fold(InhibitBits::NONE, |acc, (_, b)| acc.union(*b))
    }

    /// Live cookie count.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether no cookie is live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Which client holds a cookie (diagnostics / audit).
    #[must_use]
    pub fn holder(&self, cookie: u32) -> Option<u32> {
        self.held.get(&cookie).map(|(c, _)| *c)
    }

    /// Parse the wire bitset form (the `inhibit` request's `types`
    /// argument arrives as the ldp.core bitset).
    #[must_use]
    pub fn bits_from_bitset(bits: Bitset128) -> InhibitBits {
        InhibitBits::from_bitset(bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_lifecycle() {
        let mut reg = InhibitRegistry::new();
        let c1 = reg.take(4, InhibitBits::DISPLAY);
        let c2 = reg.take(4, InhibitBits::IDLE.union(InhibitBits::SUSPEND));
        let c3 = reg.take(9, InhibitBits::BLUR);
        assert!(c1 != 0 && c2 != 0 && c3 != 0);
        assert_eq!(reg.effective().to_wire(), 0b1111);
        assert_eq!(reg.holder(c2), Some(4));
        // Release by cookie.
        assert_eq!(reg.release(c1), Some(InhibitBits::DISPLAY));
        assert_eq!(reg.release(c1), None);
        assert_eq!(reg.effective().to_wire(), 0b1101);
        // Disconnect releases everything that client held.
        let released = reg.client_disconnected(4);
        assert_eq!(released.to_wire(), 0b1100);
        assert_eq!(reg.effective().to_wire(), 0b0001);
        assert_eq!(reg.len(), 1);
        let _ = reg.release(c3);
        assert!(reg.is_empty());
        assert_eq!(reg.effective(), InhibitBits::NONE);
    }

    #[test]
    fn wire_bits() {
        assert_eq!(InhibitBits::BLUR.to_wire(), 1);
        assert_eq!(InhibitBits::DISPLAY.to_wire(), 2);
        assert_eq!(InhibitBits::IDLE.to_wire(), 4);
        assert_eq!(InhibitBits::SUSPEND.to_wire(), 8);
        assert!(InhibitBits::from_wire(0b1001).contains(InhibitBits::SUSPEND));
        assert!(InhibitBits::from_wire(0b1001).contains(InhibitBits::BLUR));
        // Bitset interop: the wire arg arrives as ldp.core Bitset128.
        let bs = Bitset128::from_words([0b0110, 0, 0, 0]);
        let bits = InhibitRegistry::bits_from_bitset(bs);
        assert!(bits.contains(InhibitBits::DISPLAY));
        assert!(bits.contains(InhibitBits::IDLE));
        assert!(!bits.contains(InhibitBits::BLUR));
    }
}
