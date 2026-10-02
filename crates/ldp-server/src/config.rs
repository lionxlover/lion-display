//! Server configuration.
//!
//! One [`ServerConfig`] describes everything a phase-4 server core needs:
//! the release number and capability bits reported in `welcome`, the
//! advertised global interfaces, which `connection_options` the server is
//! willing to grant, the base limits, the client ceiling, the audit sink,
//! and the sandbox-verdict policy over `SO_PEERCRED` credentials.

use std::sync::Arc;

use ldp_core::bitset::Bitset128;
use ldp_core::error::{ErrorCode, LdpError};
use ldp_core::limits::Limits;
use ldp_transport::creds::PeerCreds;

use crate::audit::AuditSink;

/// `connection_options` bit indices (`spec/core.toml`).
pub mod connection_options {
    /// `registry.introspect` is enabled for the connection.
    pub const INTROSPECTION: u32 = 0;
    /// The message ceiling is raised to 64 MiB for the connection.
    pub const LARGE_MESSAGES: u32 = 1;
    /// Runtime escalation prompts may appear on this connection.
    pub const ESCALATE_PROMPTS: u32 = 2;
}

/// `server_caps` bit indices (`spec/core.toml`).
pub mod server_caps {
    /// Color management support.
    pub const COLOR_MGMT: u32 = 0;
    /// HDR support.
    pub const HDR: u32 = 1;
    /// Variable refresh rate support.
    pub const VRR: u32 = 2;
    /// Multi-GPU support.
    pub const MULTI_GPU: u32 = 3;
    /// Accessibility support.
    pub const A11Y: u32 = 4;
    /// A security broker is present.
    pub const SECURITY_BROKER: u32 = 5;
    /// Audit streaming is available.
    pub const AUDIT: u32 = 6;
}

/// How the server classifies a peer before any protocol data is read
/// (the `sandbox` verdict of `welcome`).
///
/// The default policy: same-user peers are `unconfined`, everyone else
/// `sandboxed`. A real deployment replaces this with a manifest- and
/// LSM-aware verdict (`ldp-security`, later phase).
pub type SandboxPolicy = fn(&PeerCreds) -> SandboxFlavor;

/// The `sandbox_flavor` verdict reported in `welcome` (wire values).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SandboxFlavor {
    /// Same user, no confinement observed.
    Unconfined,
    /// Legacy sandbox (pre-manifest confinement observed).
    Classic,
    /// Brokered by a desktop portal.
    Portal,
    /// Confined by an LSM/namespace sandbox.
    Sandboxed,
}

impl SandboxFlavor {
    /// The wire value of the verdict.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Unconfined => 1,
            Self::Classic => 2,
            Self::Portal => 3,
            Self::Sandboxed => 4,
        }
    }

    /// The default policy: same-user `unconfined`, otherwise `sandboxed`.
    #[must_use]
    pub fn from_creds(creds: &PeerCreds) -> SandboxFlavor {
        if creds.same_user() {
            Self::Unconfined
        } else {
            Self::Sandboxed
        }
    }
}

/// One advertised global: an interface name plus the version range the
/// server offers it at. Ranges are validated against the compiled schema
/// when the server is built.
#[derive(Clone, Debug)]
pub struct GlobalAdvert {
    /// Fully qualified interface name.
    pub interface: String,
}

impl GlobalAdvert {
    /// Advertise `interface`.
    #[must_use]
    pub fn new(interface: impl Into<String>) -> GlobalAdvert {
        GlobalAdvert {
            interface: interface.into(),
        }
    }
}

/// The phase-4 server configuration.
#[derive(Clone)]
pub struct ServerConfig {
    /// LDP release number this server speaks (reported in `welcome`).
    pub protocol_release: u32,
    /// `server_caps` bits reported in `welcome`.
    pub caps: Bitset128,
    /// Interfaces advertised as globals (what `registry.global` replays
    /// and what `registry.bind` accepts).
    pub globals: Vec<GlobalAdvert>,
    /// `connection_options` bits the server is willing to grant; anything
    /// else a client asks for is silently dropped.
    pub options_allowed: Bitset128,
    /// Base limits before `large_messages` negotiation.
    pub limits: Limits,
    /// Maximum simultaneously live client sessions.
    pub max_clients: u32,
    /// The audit sink (shared by every session thread).
    pub audit: Arc<dyn AuditSink>,
    /// Sandbox verdict policy over peer credentials.
    pub sandbox_policy: SandboxPolicy,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            protocol_release: 1,
            caps: Bitset128::EMPTY,
            globals: Vec::new(),
            options_allowed: Bitset128::single(connection_options::INTROSPECTION)
                .with(connection_options::LARGE_MESSAGES),
            limits: Limits::DEFAULT,
            max_clients: 128,
            audit: Arc::new(crate::audit::NullAudit),
            sandbox_policy: SandboxFlavor::from_creds,
        }
    }
}

impl ServerConfig {
    /// The default advertisement: every interface the schema marks
    /// `global = true`.
    #[must_use]
    pub fn all_globals() -> Vec<GlobalAdvert> {
        ldp_protocol::MODULES
            .iter()
            .flat_map(|&m| m.interfaces.iter())
            .filter(|i| i.global)
            .map(|i| GlobalAdvert::new(i.name))
            .collect()
    }

    /// Validate the configuration against the compiled schema: every
    /// advertised global must be a known interface. Unknown names would
    /// otherwise surface as confusing per-client failures.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] naming the first unknown interface (a server
    /// configuration bug, never peer input).
    pub fn validate(&self) -> Result<(), LdpError> {
        for g in &self.globals {
            if ldp_protocol::REGISTRY.interface(&g.interface).is_none() {
                return Err(LdpError::Logic {
                    what: "advertised global is not in the compiled schema",
                });
            }
        }
        if self.max_clients == 0 {
            return Err(LdpError::Logic {
                what: "max_clients must be at least 1",
            });
        }
        if self.protocol_release == 0 {
            return Err(LdpError::Logic {
                what: "protocol_release must be nonzero",
            });
        }
        Ok(())
    }

    /// Whether `interface` is bindable on this server: explicitly
    /// advertised, or the registry (always implemented, always
    /// bindable — the bootstrap global).
    #[must_use]
    pub fn advertises(&self, interface: &str) -> bool {
        interface == crate::session::REGISTRY_INTERFACE
            || self.globals.iter().any(|g| g.interface == interface)
    }

    /// The globals `get_registry` replays, in replay order: the registry
    /// itself first (unless the configuration already lists it), then
    /// the configured advertisements in order.
    #[must_use]
    pub fn replay_list(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.globals.len() + 1);
        if !self
            .globals
            .iter()
            .any(|g| g.interface == crate::session::REGISTRY_INTERFACE)
        {
            names.push(crate::session::REGISTRY_INTERFACE.to_string());
        }
        names.extend(self.globals.iter().map(|g| g.interface.clone()));
        names
    }

    /// The sandbox verdict for one peer under this configuration.
    #[must_use]
    pub fn sandbox_verdict(&self, creds: &PeerCreds) -> SandboxFlavor {
        (self.sandbox_policy)(creds)
    }

    /// Map an error to the code and message the wire `connection.error`
    /// event carries (the audit copy keeps the full detail).
    #[must_use]
    pub fn error_wire_parts(e: &LdpError) -> (ErrorCode, Option<ldp_core::ids::ObjectId>, String) {
        match e {
            LdpError::Protocol {
                code,
                object,
                message,
            } => (*code, *object, message.to_string()),
            LdpError::Malformed { code, message } => (*code, None, message.to_string()),
            LdpError::Limit { kind, value } => {
                (ErrorCode::LimitExceeded, None, format!("{kind} = {value}"))
            }
            LdpError::Io(_) => (ErrorCode::ServerError, None, "transport failure".into()),
            LdpError::TimedOut { what } | LdpError::Logic { what } => {
                (ErrorCode::ServerError, None, (*what).to_string())
            }
            _ => (ErrorCode::ServerError, None, "unclassified failure".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_globals_matches_the_schema_flag() {
        let globals = ServerConfig::all_globals();
        // Cross-checked against the generated tables: every global
        // interface is advertised, no non-global one is.
        let schema_globals: Vec<&str> = ldp_protocol::MODULES
            .iter()
            .flat_map(|&m| m.interfaces.iter())
            .filter(|i| i.global)
            .map(|i| i.name)
            .collect();
        assert_eq!(globals.len(), schema_globals.len());
        for g in &globals {
            let iface = ldp_protocol::REGISTRY.interface(&g.interface).unwrap();
            assert!(iface.global, "{} must be global", g.interface);
        }
        // The registry itself is a global (bindable like any other).
        assert!(schema_globals.contains(&"ldp.core.registry"));
    }

    #[test]
    fn validate_rejects_unknown_globals_and_zero_ceilings() {
        let mut cfg = ServerConfig {
            globals: vec![GlobalAdvert::new("ldp.nope.thing")],
            ..ServerConfig::default()
        };
        assert!(cfg.validate().is_err());
        cfg.globals = vec![GlobalAdvert::new("ldp.core.output")];
        assert!(cfg.validate().is_ok());
        cfg.max_clients = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn sandbox_flavor_wire_values_are_stable() {
        assert_eq!(SandboxFlavor::Unconfined.to_wire(), 1);
        assert_eq!(SandboxFlavor::Classic.to_wire(), 2);
        assert_eq!(SandboxFlavor::Portal.to_wire(), 3);
        assert_eq!(SandboxFlavor::Sandboxed.to_wire(), 4);
    }

    #[test]
    fn error_wire_parts_maps_every_variant() {
        let e = LdpError::protocol(
            ErrorCode::InvalidOpcode,
            Some(ldp_core::ids::ObjectId::CONNECTION),
            "nope",
        );
        let (code, object, msg) = ServerConfig::error_wire_parts(&e);
        assert_eq!(code, ErrorCode::InvalidOpcode);
        assert_eq!(object, Some(ldp_core::ids::ObjectId::CONNECTION));
        assert_eq!(msg, "nope");

        let e = LdpError::Limit {
            kind: ldp_core::error::LimitKind::ClientObjects,
            value: 7,
        };
        let (code, _, msg) = ServerConfig::error_wire_parts(&e);
        assert_eq!(code, ErrorCode::LimitExceeded);
        assert_eq!(msg, "client_objects = 7");

        let e = LdpError::Logic { what: "bug" };
        assert_eq!(ServerConfig::error_wire_parts(&e).0, ErrorCode::ServerError);
    }
}
