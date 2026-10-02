//! The tool error surface: one type every session call, tool `run`,
//! and example shares.
//!
//! The layers below each own an error vocabulary — [`LdpError`] in the
//! core taxonomy, [`ClientError`] on the client side (disconnects and
//! server-fatal errors ride alongside) — and the tools add their own
//! failure families: local file/syscall I/O and the *honest-degradation*
//! refusals (`ToolError::Logic` — "this server does not advertise that
//! interface", "the wait expired"). Rather than flatten those into the
//! core taxonomy (which cannot carry dynamic diagnostics in its
//! `Logic`/`TimedOut` arms by design — those are for invariant bugs,
//! not tool reports), the toolchain keeps its own enum with `From`
//! bridges from every layer it calls, so `?` works at every seam.
//!
//! Usage problems are *not* here: argv mistakes are
//! [`crate::args::UsageError`] (exit code 2, printed with the usage
//! text), while everything in this type is a reportable failure
//! (exit code 1).

#![forbid(unsafe_code)]

use std::fmt;

use ldp_client::error::ClientError;
use ldp_core::error::LdpError;

use crate::sys::SysError;

/// Everything a tool run reports as a failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum ToolError {
    /// Underlying client / protocol / transport failure: connect and
    /// handshake errors, bind refusals from the server, codec and
    /// framing failures, disconnects, server-fatal `connection.error`.
    Client(ClientError),
    /// Local I/O failure: memfd creation, trace-file read/write, audit
    /// log files, socket path resolution gone wrong at the OS level.
    Io(std::io::Error),
    /// A local precondition failed — the honest-degradation path. The
    /// message is fit for operators (it names what was wanted and what
    /// was found) but is not a protocol participant.
    Logic(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(e) => write!(f, "client: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Logic(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Logic(_) => None,
        }
    }
}

impl From<ClientError> for ToolError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

impl From<LdpError> for ToolError {
    fn from(e: LdpError) -> Self {
        Self::Client(ClientError::Ldp(e))
    }
}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SysError> for ToolError {
    fn from(e: SysError) -> Self {
        Self::Io(std::io::Error::other(e))
    }
}

/// The toolchain result type.
pub type Result<T, E = ToolError> = core::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_displays_nonempty() {
        let cases = vec![
            ToolError::Client(ClientError::IdExhausted),
            ToolError::Client(ClientError::Ldp(LdpError::TimedOut { what: "fence" })),
            ToolError::Io(std::io::Error::other("gone")),
            ToolError::Logic("interface 'x' is not advertised".to_owned()),
        ];
        for c in &cases {
            assert!(!c.to_string().is_empty());
        }
    }

    #[test]
    fn bridges_work_at_the_question_mark_seams() {
        fn fallible() -> Result<()> {
            Err(std::io::Error::other("nope"))?;
            Ok(())
        }
        fn client() -> Result<()> {
            Err(ClientError::ConnectionDead)?;
            Ok(())
        }
        fn core() -> Result<()> {
            Err(LdpError::Logic { what: "bug" })?;
            Ok(())
        }
        assert!(matches!(fallible(), Err(ToolError::Io(_))));
        assert!(matches!(client(), Err(ToolError::Client(_))));
        assert!(matches!(
            core(),
            Err(ToolError::Client(ClientError::Ldp(LdpError::Logic { .. })))
        ));
    }
}
