//! Socket discovery for live tool modes.
//!
//! `lion-compositor` listens on an abstract-namespace Unix socket; the
//! server side takes `--socket NAME` (no leading NUL) and defaults to
//! `lion-compositor-<pid>`. The tool side resolves the same name in
//! this order:
//!
//! 1. the tool's `--socket NAME` flag (a leading `@` is accepted and
//!    stripped — the conventional abstract-socket sigil),
//! 2. the `LDP_SOCKET` environment variable (same format),
//! 3. neither: a usage error that names the convention.
//!
//! There is deliberately no default name: the compositor's own default
//! embeds its pid, and guessing would turn a clear error into a
//! mysterious connection refusal.

#![forbid(unsafe_code)]

use ldp_transport::UnixAddr;

use crate::args::UsageError;

/// Resolve a live-mode socket name into an abstract [`UnixAddr`].
///
/// `explicit` is the `--socket` flag value when present; `env` is the
/// process environment (pass [`std::env::var_os`] results in as
/// `Option<&str>` — tests inject both sides).
///
/// # Errors
/// [`UsageError`] when neither source provides a name, or the name is
/// unusable as an abstract socket name.
pub fn resolve(explicit: Option<&str>, env: Option<&str>) -> Result<UnixAddr, UsageError> {
    let Some(name) = explicit.or(env) else {
        return Err(UsageError(
            "no socket name: pass --socket NAME or set LDP_SOCKET \
             (the compositor's --socket value; abstract namespace)"
                .to_owned(),
        ));
    };
    let name = name.strip_prefix('@').unwrap_or(name);
    if name.is_empty() {
        return Err(UsageError("socket name is empty".to_owned()));
    }
    UnixAddr::abstract_name(name.as_bytes())
        .map_err(|e| UsageError(format!("socket name '{name}' is unusable: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_flag_wins_and_at_is_stripped() {
        let addr = resolve(Some("@lion-tools"), Some("from-env")).unwrap();
        assert_eq!(addr.display_string(), "@lion-tools");
    }

    #[test]
    fn env_is_the_fallback() {
        let addr = resolve(None, Some("lion-env")).unwrap();
        assert_eq!(addr.display_string(), "@lion-env");
    }

    #[test]
    fn neither_is_a_usage_error_naming_the_convention() {
        let err = resolve(None, None).unwrap_err();
        assert!(err.0.contains("--socket"));
        assert!(err.0.contains("LDP_SOCKET"));
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(resolve(Some(""), None).is_err());
        assert!(resolve(Some("@"), None).is_err());
    }

    #[test]
    fn overlong_name_is_rejected() {
        let long = "x".repeat(200);
        assert!(resolve(Some(&long), None).is_err());
    }
}
