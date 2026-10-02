//! Application manifests — the launch-time baseline of the layered policy.
//!
//! A manifest is the app's *asked-for* scope set, verified by the broker
//! before the app connects (`docs/architecture.md` §16.1). The canonical
//! serialization below is what the manifest hash covers: the broker's
//! audit records quote that hash, so a grant can always be traced to the
//! exact manifest text that justified it — the threat model's
//! "justification (manifest hash or prompt id)" column.
//!
//! Canonical form (all fields in fixed order, NUL-terminated, little-endian):
//!
//! ```text
//! b"ldp-manifest-v1" 0x00 app_id 0x00 version:u32 scopes:4x u32 sandbox:u32
//! ```
//!
//! Scope words are the [`ScopeSet::to_words`] form; sandbox is the wire
//! value. The format is versioned by the leading tag so a future schema
//! change is detectable, not silently mis-hashed.

use ldp_core::caps::{SandboxFlavor, ScopeSet};

use crate::sha256::{hex32, sha256};

/// Manifest validation failures (machine-stable labels; the wire message
/// for a rejected manifest is the broker's refusal to register it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestError {
    /// app_id outside `[a-z0-9][a-z0-9.-]{0,127}`.
    InvalidAppId,
    /// Manifests start at version 1.
    ZeroVersion,
    /// app_id longer than 128 bytes.
    AppIdTooLong,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAppId => f.write_str("invalid app_id"),
            Self::ZeroVersion => f.write_str("zero version"),
            Self::AppIdTooLong => f.write_str("app_id too long"),
        }
    }
}

impl std::error::Error for ManifestError {}

/// One application's launch manifest.
///
/// Holds no secrets; it is public metadata (the app ships it). The broker
/// keeps the registered copy; clients never send it over the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppManifest {
    /// Reverse-DNS-ish identifier, charset `[a-z0-9.-]`.
    pub app_id: String,
    /// Monotonically increasing app data version (>= 1).
    pub version: u32,
    /// Scopes the app asks for at launch.
    pub requested: ScopeSet,
    /// Declared sandbox flavor (audit context; also feeds policy — a
    /// fully sandboxed app's manifests are trusted further than an
    /// unconfined one's in a future hardening pass).
    pub sandbox: SandboxFlavor,
}

impl AppManifest {
    /// Validate the invariants that do not involve the broker's state.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] for a malformed id or version. The scope set
    /// cannot be invalid — [`ScopeSet`] is structurally sound.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version == 0 {
            return Err(ManifestError::ZeroVersion);
        }
        if self.app_id.len() > 128 {
            return Err(ManifestError::AppIdTooLong);
        }
        // Byte-level checks: the allowed charset is pure ASCII, so any
        // non-ASCII byte (multi-byte UTF-8) fails without slicing at a
        // char boundary.
        let bytes = self.app_id.as_bytes();
        let Some(&first) = bytes.first() else {
            return Err(ManifestError::InvalidAppId);
        };
        let ok_first = first.is_ascii_lowercase() || first.is_ascii_digit();
        let ok_rest = bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
        if !ok_first || !ok_rest {
            return Err(ManifestError::InvalidAppId);
        }
        Ok(())
    }

    /// The canonical byte form covered by the manifest hash.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.app_id.len());
        out.extend_from_slice(b"ldp-manifest-v1\x00");
        out.extend_from_slice(self.app_id.as_bytes());
        out.push(0);
        out.extend_from_slice(&self.version.to_le_bytes());
        for word in self.requested.to_words() {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&self.sandbox.to_wire().to_le_bytes());
        out
    }

    /// SHA-256 over the canonical form (the audit-trail identity of this
    /// exact manifest).
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        sha256(&self.canonical_bytes())
    }

    /// Hash as lowercase hex (audit detail fields quote this form).
    #[must_use]
    pub fn hash_hex(&self) -> String {
        hex32(&self.hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::caps::Scope;

    fn manifest(app: &str, version: u32, scopes: ScopeSet) -> AppManifest {
        AppManifest {
            app_id: app.to_owned(),
            version,
            requested: scopes,
            sandbox: SandboxFlavor::Sandboxed,
        }
    }

    #[test]
    fn app_id_charset() {
        let ok = ScopeSet::single(Scope::ClipboardRead);
        assert!(manifest("org.example.app", 1, ok).validate().is_ok());
        assert!(manifest("a", 1, ok).validate().is_ok());
        assert!(manifest("0.app-1", 3, ok).validate().is_ok());
        for bad in ["", ".lead", "-lead", "Upper", "sp ace", "sla/sh", "ütf"] {
            assert_eq!(
                manifest(bad, 1, ok).validate(),
                Err(ManifestError::InvalidAppId),
                "{bad:?}"
            );
        }
        assert_eq!(
            manifest(&"x".repeat(129), 1, ok).validate(),
            Err(ManifestError::AppIdTooLong)
        );
        assert_eq!(manifest(&"x".repeat(128), 1, ok).validate(), Ok(()));
        assert_eq!(
            manifest("ok", 0, ok).validate(),
            Err(ManifestError::ZeroVersion)
        );
    }

    #[test]
    fn canonical_form_is_field_ordered_and_stable() {
        let a = manifest("org.example.app", 7, ScopeSet::single(Scope::Screenshot));
        let bytes = a.canonical_bytes();
        assert!(bytes.starts_with(b"ldp-manifest-v1\x00"));
        assert!(bytes.ends_with(&SandboxFlavor::Sandboxed.to_wire().to_le_bytes()));
        // Any field change flips the hash.
        let mut b = a.clone();
        b.version = 8;
        assert_ne!(a.hash(), b.hash());
        b = a.clone();
        b.requested = ScopeSet::NONE;
        assert_ne!(a.hash(), b.hash());
        b = a.clone();
        b.sandbox = SandboxFlavor::Unconfined;
        assert_ne!(a.hash(), b.hash());
        b = a.clone();
        b.app_id.push('x');
        assert_ne!(a.hash(), b.hash());
        // Equal manifests agree.
        assert_eq!(
            a.hash(),
            manifest("org.example.app", 7, ScopeSet::single(Scope::Screenshot)).hash()
        );
    }

    #[test]
    fn hash_hex_shape() {
        let a = manifest("org.example.app", 1, ScopeSet::NONE);
        let h = a.hash_hex();
        assert_eq!(h.len(), 64);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}
