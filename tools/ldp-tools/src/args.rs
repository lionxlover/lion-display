//! The tiny argv scanner shared by the six tool binaries.
//!
//! Hand-rolled like every other CLI in this workspace (no clap): flags
//! are matched by exact name, values by `--name value` or
//! `--name=value`. Flags and their values are *consumed* as they are
//! recognized; everything left over is either a positional argument or
//! an unknown argument — callers decide which via
//! [`Args::take_positional`] and [`Args::require_empty`].
//!
//! One flag is shared by every shipped binary: `--version` / `-V`,
//! consumed via [`Args::take_version_flag`], prints
//! `<tool> <CARGO_PKG_VERSION>` (the workspace release version) and
//! exits 0 — the uniform version surface `scripts/version_check.py`
//! and the live tool suites pin.

#![forbid(unsafe_code)]

/// A usage problem (unknown flag, missing value, leftover argument).
///
/// Tools report this as exit code 2 with the message on stderr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// A scanning argv: recognized flags and values are consumed, the
/// remainder is positional.
#[derive(Clone, Debug, Default)]
pub struct Args {
    items: Vec<String>,
}

impl Args {
    /// Collect the process arguments (argv without `argv[0]`).
    #[must_use]
    pub fn from_env() -> Args {
        Args::from_argv(std::env::args().skip(1))
    }

    /// Build from an explicit argument slice (tests drive the library
    /// entry points the same way the binaries do).
    #[must_use]
    pub fn from_argv<I, S>(items: I) -> Args
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Args {
            items: items.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether `--name` is present; consumes it.
    pub fn take_flag(&mut self, name: &str) -> bool {
        let Some(pos) = self.items.iter().position(|a| a == name) else {
            return false;
        };
        self.items.remove(pos);
        true
    }

    /// Whether `--version` or `-V` is present; consumes it.
    ///
    /// The one flag every shipped binary shares: adapters print
    /// `<tool> <CARGO_PKG_VERSION>` and exit 0, so the whole toolchain
    /// reports the workspace release version uniformly.
    pub fn take_version_flag(&mut self) -> bool {
        self.take_flag("--version") || self.take_flag("-V")
    }

    /// The value of `--name VALUE` / `--name=VALUE`; consumes both.
    ///
    /// # Errors
    /// [`UsageError`] when the flag is present but has no value.
    pub fn take_value(&mut self, name: &str) -> Result<Option<String>, UsageError> {
        let joined = format!("{name}=");
        for pos in 0..self.items.len() {
            if self.items[pos] == name {
                let Some(value) = self.items.get(pos + 1).cloned() else {
                    return Err(UsageError(format!("{name} needs a value")));
                };
                self.items.remove(pos + 1);
                self.items.remove(pos);
                return Ok(Some(value));
            }
            if let Some(value) = self.items[pos].strip_prefix(&joined) {
                let value = value.to_owned();
                self.items.remove(pos);
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    /// Every value of a repeatable flag, in order; consumes all of them.
    ///
    /// # Errors
    /// [`UsageError`] when any occurrence lacks a value.
    pub fn take_values(&mut self, name: &str) -> Result<Vec<String>, UsageError> {
        let mut out = Vec::new();
        while let Some(v) = self.take_value(name)? {
            out.push(v);
        }
        Ok(out)
    }

    /// The first positional argument, consumed.
    #[must_use]
    pub fn take_positional(&mut self) -> Option<String> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.items.remove(0))
        }
    }

    /// The unconsumed arguments (positional arguments, in order).
    #[must_use]
    pub fn positionals(&self) -> &[String] {
        &self.items
    }

    /// Reject any unconsumed argument.
    ///
    /// # Errors
    /// [`UsageError`] naming the first leftover.
    pub fn require_empty(&self) -> Result<(), UsageError> {
        if let Some(item) = self.items.first() {
            return Err(UsageError(format!(
                "unknown argument '{item}' (try --help)"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_values_round_trip() {
        let mut args = Args::from_argv(["--live", "--socket", "@lion", "--frames=8", "--live"]);
        assert!(args.take_flag("--live"));
        assert!(args.take_flag("--live"));
        assert!(!args.take_flag("--live"));
        assert_eq!(
            args.take_value("--socket").unwrap(),
            Some("@lion".to_owned())
        );
        assert_eq!(args.take_value("--frames").unwrap(), Some("8".to_owned()));
        assert_eq!(args.take_value("--missing").unwrap(), None);
        args.require_empty().unwrap();
    }

    #[test]
    fn positionals_survive_flag_consumption() {
        let mut args = Args::from_argv(["--bind", "ldp.core.output", "trace.bin"]);
        assert_eq!(
            args.take_value("--bind").unwrap(),
            Some("ldp.core.output".to_owned())
        );
        assert_eq!(args.take_positional(), Some("trace.bin".to_owned()));
        assert!(args.take_positional().is_none());
        args.require_empty().unwrap();
    }

    #[test]
    fn missing_value_is_a_usage_error() {
        let mut args = Args::from_argv(["--socket"]);
        assert_eq!(
            args.take_value("--socket").unwrap_err().0,
            "--socket needs a value"
        );
    }

    #[test]
    fn version_flag_is_consumed() {
        let mut args = Args::from_argv(["--version"]);
        assert!(args.take_version_flag());
        args.require_empty().unwrap();

        let mut args = Args::from_argv(["-V", "--live"]);
        assert!(args.take_version_flag());
        assert!(args.take_flag("--live"));
        args.require_empty().unwrap();

        let mut args = Args::from_argv(["--live"]);
        assert!(!args.take_version_flag());
        assert!(args.take_flag("--live"));
    }

    #[test]
    fn leftovers_are_rejected() {
        let mut args = Args::from_argv(["--live", "--nonsense"]);
        assert!(args.take_flag("--live"));
        let err = args.require_empty().unwrap_err();
        assert!(err.0.contains("--nonsense"));
    }

    #[test]
    fn repeated_values_keep_order() {
        let mut args = Args::from_argv(["--bind", "ldp.core.output", "--bind", "ldp.core.shm"]);
        assert_eq!(
            args.take_values("--bind").unwrap(),
            vec!["ldp.core.output".to_owned(), "ldp.core.shm".to_owned()]
        );
        args.require_empty().unwrap();
    }
}
