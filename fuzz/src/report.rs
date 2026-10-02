//! The shared target report.

/// One fuzz target's outcome.
///
/// `accepted`/`rejected` count *input dispositions*: how many inputs
/// survived validation vs. were rejected with a structured error. A
/// healthy run must show **both** — an all-accept run never reached
/// the validation paths, an all-reject run never generated valid
/// input; either means the harness is broken, not the target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetReport {
    /// Target name (`"codec"`).
    pub target: &'static str,
    /// Inputs processed.
    pub iterations: u64,
    /// Inputs that passed validation (decoded / framed / served).
    pub accepted: u64,
    /// Inputs rejected with a structured error.
    pub rejected: u64,
}

impl TargetReport {
    /// Both validation paths exercised?
    #[must_use]
    pub fn sane(&self) -> bool {
        self.iterations > 0 && self.accepted > 0 && self.rejected > 0
    }

    /// One-line human summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{}: {} iterations, {} accepted, {} rejected",
            self.target, self.iterations, self.accepted, self.rejected
        )
    }
}
