//! Capability access tokens.
//!
//! 256-bit unforgeable bearer tokens minted by the session broker
//! (`docs/architecture.md` §16). Clients receive them out-of-band (grant
//! event or portal) and submit them via `ldp.security.submit_token`. The
//! broker validates against its grant table; the server never stores raw
//! token bytes in logs — [`AccessToken`] deliberately renders redacted.

use crate::caps::ScopeSet;
use crate::time::Mono;
use core::fmt;

/// Token length in bytes (256-bit).
pub const TOKEN_BYTES: usize = 32;

/// Token length in u32 words (wire form: 8 words).
pub const TOKEN_WORDS: usize = 8;

/// A 256-bit capability token with its grant metadata.
///
/// Security properties:
///
/// * **Unforgeable**: 2^256 random; guessing is not an attack.
/// * **Timing-safe comparison**: [`AccessToken::constant_time_eq`] folds
///   XOR across all bytes — no early exit.
/// * **Never logged**: [`fmt::Debug`]/[`fmt::Display`] render only scope
///   and expiry, never the raw bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken {
    bytes: [u8; TOKEN_BYTES],
    scope: ScopeSet,
    expires_at: Option<Mono>,
}

// Deliberate redaction: the secret bytes must never appear in Debug/Display
// output (they would leak through logs); clippy's completeness check is
// silenced here on purpose.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("scope", &self.scope.to_string())
            .field("expires_at", &self.expires_at.map(Mono::as_ns))
            .field("token", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccessToken(<redacted>, scope={})", self.scope)
    }
}

impl AccessToken {
    /// Mint from raw bytes (broker side). Accepts exactly
    /// [`TOKEN_BYTES`] bytes.
    pub fn from_bytes(
        bytes: &[u8],
        scope: ScopeSet,
        expires_at: Option<Mono>,
    ) -> Option<AccessToken> {
        if bytes.len() != TOKEN_BYTES {
            return None;
        }
        let mut buf = [0u8; TOKEN_BYTES];
        buf.copy_from_slice(bytes);
        Some(AccessToken {
            bytes: buf,
            scope,
            expires_at,
        })
    }

    /// Mint from the 8-word wire form.
    pub fn from_words(
        words: [u32; TOKEN_WORDS],
        scope: ScopeSet,
        expires_at: Option<Mono>,
    ) -> AccessToken {
        let mut bytes = [0u8; TOKEN_BYTES];
        for (i, w) in words.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        AccessToken {
            bytes,
            scope,
            expires_at,
        }
    }

    /// Raw bytes — only for the transport toward the verifying party.
    /// Handle as secret material.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; TOKEN_BYTES] {
        &self.bytes
    }

    /// The 8-word wire form.
    #[must_use]
    pub fn to_words(&self) -> [u32; TOKEN_WORDS] {
        let mut out = [0u32; TOKEN_WORDS];
        for (i, chunk) in self.bytes.chunks_exact(4).enumerate() {
            out[i] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        out
    }

    /// The scope set this token grants.
    #[must_use]
    pub const fn scope(&self) -> ScopeSet {
        self.scope
    }

    /// Expiry (monotonic); `None` = session-bound (valid until the
    /// connection it was submitted on dies).
    #[must_use]
    pub const fn expires_at(&self) -> Option<Mono> {
        self.expires_at
    }

    /// Whether the token is still valid at `now`.
    #[must_use]
    pub fn is_valid_at(&self, now: Mono) -> bool {
        match self.expires_at {
            Some(exp) => now <= exp,
            None => true,
        }
    }

    /// Constant-time equality of the raw token bytes.
    ///
    /// Every byte is folded into the result; the function returns in
    /// fixed time regardless of where the first difference sits. The
    /// scope/expiry fields are compared normally (they are public).
    #[must_use]
    pub fn constant_time_eq(&self, other: &AccessToken) -> bool {
        let mut diff: u8 = 0;
        for (a, b) in self.bytes.iter().zip(other.bytes.iter()) {
            diff |= a ^ b;
        }
        diff == 0 && self.scope == other.scope && self.expires_at == other.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Scope;

    fn token(seed: u8, scope: ScopeSet) -> AccessToken {
        let bytes = [seed; TOKEN_BYTES];
        AccessToken::from_bytes(&bytes, scope, None).unwrap()
    }

    #[test]
    fn byte_and_word_forms_agree() {
        let bytes: [u8; TOKEN_BYTES] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let scope = ScopeSet::single(Scope::Screenshot);
        let t = AccessToken::from_bytes(&bytes, scope, Some(Mono::from_ns(1234))).unwrap();
        let words = t.to_words();
        let t2 = AccessToken::from_words(words, scope, Some(Mono::from_ns(1234)));
        assert!(t.constant_time_eq(&t2));
        assert_eq!(t.to_words(), t2.to_words());
    }

    #[test]
    fn length_is_enforced() {
        assert!(AccessToken::from_bytes(&[0u8; 31], ScopeSet::NONE, None).is_none());
        assert!(AccessToken::from_bytes(&[0u8; 33], ScopeSet::NONE, None).is_none());
        assert!(AccessToken::from_bytes(&[0u8; 32], ScopeSet::NONE, None).is_some());
        assert_eq!(TOKEN_WORDS, TOKEN_BYTES / 4);
    }

    #[test]
    fn equality_is_byte_exact() {
        let a = token(0xAA, ScopeSet::NONE);
        let same = token(0xAA, ScopeSet::NONE);
        let other = token(0xAB, ScopeSet::NONE);
        assert!(a.constant_time_eq(&same));
        assert!(!a.constant_time_eq(&other));
        // Same bytes, different scope: not equal.
        let scoped =
            AccessToken::from_bytes(&[0xAA; TOKEN_BYTES], ScopeSet::single(Scope::Bridge), None)
                .unwrap();
        assert!(!a.constant_time_eq(&scoped));
    }

    #[test]
    fn expiry_gate() {
        let exp = Mono::from_ns(1_000);
        let t = AccessToken::from_bytes(&[1; TOKEN_BYTES], ScopeSet::NONE, Some(exp)).unwrap();
        assert!(t.is_valid_at(Mono::from_ns(999)));
        assert!(t.is_valid_at(exp));
        assert!(!t.is_valid_at(Mono::from_ns(1_001)));
        assert!(token(1, ScopeSet::NONE).is_valid_at(Mono::from_ns(u64::MAX)));
    }

    #[test]
    fn secrets_never_render() {
        let t = AccessToken::from_bytes(
            &[0x5A; TOKEN_BYTES],
            ScopeSet::single(Scope::Screenshot),
            None,
        )
        .unwrap();
        let dbg = format!("{t:?}");
        let disp = format!("{t}");
        assert!(!dbg.contains("5A"), "debug leaked token bytes");
        assert!(!disp.contains("5A"), "display leaked token bytes");
        assert!(dbg.contains("<redacted>"));
        assert!(disp.contains("screenshot"));
    }
}
