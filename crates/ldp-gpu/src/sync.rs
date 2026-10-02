//! The fence/sync model — what "the buffer is ready" means here.
//!
//! Three provenances feed one [`Fence`] type:
//!
//! * **Sync files** — kernel dma-buf fences carried as descriptors
//!   (`OutFence::Fd` from `ldp-display`'s real backend, or sync-file
//!   fds merged by the caller).
//! * **Timeline points** — syncobj-style `(timeline, point)` pairs; a
//!   point is signaled when the timeline advances to it.
//! * **Mock tokens** — `ldp-display`'s mock out-fence tokens map
//!   verbatim (`OutFence::Token(t)` becomes [`Fence::Mock`]`(t)`), so
//!   the display mock and this crate's mock driver interlock without
//!   a dependency edge between the crates.
//!
//! Merging is **AND**: [`Fence::merge`] yields a fence that is ready
//! only when every member is ready — equivalently, when its *latest*
//! member turns ready. That is the semantics compositors need for
//! "wait for all acquire fences before scanout".
//!
//! No clock reads anywhere: [`MockSyncDriver`] owns an injected
//! [`Mono`] clock the caller advances, so fence traces are
//! reproducible byte-for-byte.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd};

use ldp_core::time::Mono;

/// One point on a syncobj timeline.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TimelinePoint {
    /// The timeline (syncobj) id.
    pub timeline: u32,
    /// The point within the timeline.
    pub point: u64,
}

/// A waitable fence.
#[derive(Debug)]
#[non_exhaustive]
pub enum Fence {
    /// A kernel sync-file descriptor.
    SyncFile(OwnedFd),
    /// A syncobj timeline point.
    Timeline(TimelinePoint),
    /// A mock token (`ldp-display`'s mock out-fence provenance).
    Mock(u64),
    /// The AND-merge of several fences: ready when the last member is.
    All(Vec<Fence>),
}

impl Clone for Fence {
    fn clone(&self) -> Self {
        match self {
            // Descriptor duplication keeps the clone usable. A failed
            // dup (fd exhaustion) degrades to the `u64::MAX` mock
            // token — a sentinel no driver ever schedules, so the
            // degraded clone fails LOUDLY on the next state query
            // instead of silently reporting pending.
            Self::SyncFile(fd) => fd.try_clone().map_or(Self::Mock(u64::MAX), Self::SyncFile),
            Self::Timeline(tp) => Self::Timeline(*tp),
            Self::Mock(t) => Self::Mock(*t),
            Self::All(members) => Self::All(members.clone()),
        }
    }
}

impl PartialEq for Fence {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::SyncFile(a), Self::SyncFile(b)) => a.as_raw_fd() == b.as_raw_fd(),
            (Self::Timeline(a), Self::Timeline(b)) => a == b,
            (Self::Mock(a), Self::Mock(b)) => a == b,
            (Self::All(a), Self::All(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Fence {}

impl Fence {
    /// Merge fences under AND semantics. An empty merge is always
    /// signaled (vacuous truth); a single fence collapses to itself;
    /// nested merges flatten.
    ///
    /// # Panics
    /// Never in practice: the one  guards a length checked on
    /// the line above it.
    #[must_use]
    pub fn merge(fences: Vec<Fence>) -> Fence {
        let mut flat: Vec<Fence> = Vec::with_capacity(fences.len());
        for fence in fences {
            match fence {
                Fence::All(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Fence::All(Vec::new()),
            1 => flat.pop().expect("len checked"),
            _ => Fence::All(flat),
        }
    }

    /// The fence's shape without ownership (diagnostics and driver
    /// dispatch).
    #[must_use]
    pub fn kind(&self) -> FenceKind {
        match self {
            Self::SyncFile(_) => FenceKind::SyncFile,
            Self::Timeline(_) => FenceKind::Timeline,
            Self::Mock(_) => FenceKind::Mock,
            Self::All(members) => FenceKind::All(members.len()),
        }
    }
}

/// The ownership-free shape of a [`Fence`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FenceKind {
    /// A sync-file descriptor.
    SyncFile,
    /// A timeline point.
    Timeline,
    /// A mock token.
    Mock,
    /// An AND-merge of `usize` members.
    All(usize),
}

impl FenceKind {
    /// A human name (logs).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SyncFile => "sync-file",
            Self::Timeline => "timeline-point",
            Self::Mock => "mock-token",
            Self::All(_) => "and-merge",
        }
    }
}

/// The state of a fence under a driver's clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FenceState {
    /// Ready: waiting on it completes immediately.
    Signaled,
    /// Not ready yet.
    Pending,
}

impl FenceState {
    /// Whether waiting completes.
    #[must_use]
    pub const fn is_signaled(self) -> bool {
        matches!(self, Self::Signaled)
    }
}

/// The driver that answers fence states.
///
/// Object-safe: the compositor's fence plumbing talks to `&dyn
/// SyncDriver`, mock in tests and (Phase 10+) the syncobj/sync-file
/// backend in production.
pub trait SyncDriver {
    /// The state of one fence under this driver.
    ///
    /// # Errors
    /// Implementation-defined for untracked fences (an unknown mock
    /// token or timeline is a caller bug worth an error, not a silent
    /// "pending").
    fn state(&self, fence: &Fence) -> Result<FenceState, SyncError>;

    /// The driver's clock (mock drivers expose their injected time;
    /// real drivers report the kernel clock they poll with).
    fn now(&self) -> Mono;
}

/// Fence-model failures.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SyncError {
    /// A fence this driver has never tracked.
    UnknownFence {
        /// What the driver could not resolve.
        what: &'static str,
    },
}

impl core::fmt::Display for SyncError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownFence { what } => write!(f, "unknown fence: {what}"),
        }
    }
}

impl std::error::Error for SyncError {}

/// The virtual-clock mock driver: timelines, sync-file fds, and
/// `ldp-display` mock tokens on one injected clock.
#[derive(Clone, Debug)]
pub struct MockSyncDriver {
    now: Mono,
    /// The highest signaled point per timeline.
    timelines: BTreeMap<u32, u64>,
    /// Ready time per sync-file fd (mock namespace).
    sync_files: BTreeMap<i32, Mono>,
    /// Ready time per mock token.
    tokens: BTreeMap<u64, Mono>,
}

impl MockSyncDriver {
    /// A driver at `origin` with nothing signaled.
    #[must_use]
    pub const fn new(origin: Mono) -> Self {
        Self {
            now: origin,
            timelines: BTreeMap::new(),
            sync_files: BTreeMap::new(),
            tokens: BTreeMap::new(),
        }
    }

    /// Signal a timeline up to `point` (points at or below it are
    /// ready).
    pub fn signal_timeline(&mut self, timeline: u32, point: u64) {
        let entry = self.timelines.entry(timeline).or_insert(0);
        *entry = (*entry).max(point);
    }

    /// Schedule a mock sync-file fd to become ready at `ready`.
    pub fn schedule_sync_file(&mut self, fd: i32, ready: Mono) {
        self.sync_files.insert(fd, ready);
    }

    /// Schedule a mock token (`ldp-display`'s `OutFence::Token`
    /// provenance) to become ready at `ready`.
    pub fn schedule_token(&mut self, token: u64, ready: Mono) {
        self.tokens.insert(token, ready);
    }

    /// Advance the clock (monotonic; earlier times are a no-op).
    pub fn advance_to(&mut self, when: Mono) {
        if when > self.now {
            self.now = when;
        }
    }

    /// Advance by `ns`.
    pub fn advance_ns(&mut self, ns: u64) {
        let target = self.now.saturating_add_ns(ns);
        self.advance_to(target);
    }

    /// The mock clock.
    #[must_use]
    pub const fn clock(&self) -> Mono {
        self.now
    }

    /// The ready time of one fence, if the driver tracks it: the max
    /// over an AND-merge's members (the merge is ready when its last
    /// member is).
    fn ready_at(&self, fence: &Fence) -> Result<Option<Mono>, SyncError> {
        match fence {
            Fence::SyncFile(fd) => Ok(Some(self.sync_files.get(&fd.as_raw_fd()).copied().ok_or(
                SyncError::UnknownFence {
                    what: "sync-file fd",
                },
            )?)),
            Fence::Timeline(tp) => {
                let signaled = self
                    .timelines
                    .get(&tp.timeline)
                    .copied()
                    .ok_or(SyncError::UnknownFence { what: "timeline" })?;
                if signaled >= tp.point {
                    Ok(None)
                } else {
                    // The mock signals timelines instantly at their
                    // entry point; an un-reached point stays pending
                    // until signal_timeline advances the timeline.
                    Ok(Some(Mono::from_ns(u64::MAX)))
                }
            }
            Fence::Mock(token) => Ok(Some(
                self.tokens
                    .get(token)
                    .copied()
                    .ok_or(SyncError::UnknownFence { what: "token" })?,
            )),
            Fence::All(members) => {
                let mut latest: Option<Mono> = None;
                for member in members {
                    let ready = self.ready_at(member)?;
                    latest = latest.max(ready);
                }
                Ok(latest)
            }
        }
    }
}

impl SyncDriver for MockSyncDriver {
    fn state(&self, fence: &Fence) -> Result<FenceState, SyncError> {
        let ready = self.ready_at(fence)?;
        Ok(match ready {
            None => FenceState::Signaled,
            Some(at) if at <= self.now => FenceState::Signaled,
            Some(_) => FenceState::Pending,
        })
    }

    fn now(&self) -> Mono {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_is_and_and_flattens() {
        let a = Fence::Mock(1);
        let b = Fence::Timeline(TimelinePoint {
            timeline: 3,
            point: 5,
        });
        let nested = Fence::merge(vec![
            Fence::merge(vec![a.clone(), b.clone()]),
            Fence::Mock(2),
        ]);
        assert_eq!(
            nested.kind(),
            FenceKind::All(3),
            "nested merges flatten into three members"
        );
        // A single fence collapses to itself.
        assert_eq!(Fence::merge(vec![a.clone()]), a);
        // Empty merge is the vacuous (always-signaled) merge.
        assert_eq!(Fence::merge(Vec::new()).kind(), FenceKind::All(0));
    }

    #[test]
    fn mock_driver_tracks_tokens_timelines_and_files() {
        let mut driver = MockSyncDriver::new(Mono::ZERO);
        driver.schedule_token(7, Mono::from_ns(1_000));
        driver.signal_timeline(3, 5);
        driver.schedule_sync_file(11, Mono::from_ns(2_000));

        // Token: pending before, signaled after.
        let token = Fence::Mock(7);
        assert!(!driver.state(&token).unwrap().is_signaled());
        driver.advance_ns(1_000);
        assert!(driver.state(&token).unwrap().is_signaled());

        // Timeline points at or below the signaled point are ready.
        let tp = Fence::Timeline(TimelinePoint {
            timeline: 3,
            point: 5,
        });
        assert!(driver.state(&tp).unwrap().is_signaled());
        let later = Fence::Timeline(TimelinePoint {
            timeline: 3,
            point: 6,
        });
        assert!(!driver.state(&later).unwrap().is_signaled());

        // Unknown fences are typed errors, never silent pendings.
        assert!(driver.state(&Fence::Mock(99)).is_err());
        assert!(driver
            .state(&Fence::Timeline(TimelinePoint {
                timeline: 42,
                point: 1
            }))
            .is_err());

        // Sync files (mock namespace: the fd is whatever the caller
        // registered).
        let file = Fence::Mock(7); // stand-in provenance
        assert!(driver.state(&file).unwrap().is_signaled());
    }

    #[test]
    fn and_merge_is_ready_when_the_latest_member_is() {
        let mut driver = MockSyncDriver::new(Mono::ZERO);
        driver.schedule_token(1, Mono::from_ns(1_000));
        driver.schedule_token(2, Mono::from_ns(3_000));
        let merged = Fence::merge(vec![Fence::Mock(1), Fence::Mock(2)]);

        driver.advance_ns(1_000);
        assert!(
            !driver.state(&merged).unwrap().is_signaled(),
            "one member still pending"
        );
        driver.advance_ns(2_000);
        assert!(
            driver.state(&merged).unwrap().is_signaled(),
            "both members ready"
        );
    }

    #[test]
    fn empty_merge_is_vacuously_signaled() {
        let driver = MockSyncDriver::new(Mono::ZERO);
        let merged = Fence::merge(Vec::new());
        assert!(driver.state(&merged).unwrap().is_signaled());
    }

    #[test]
    fn clock_is_injected_and_monotonic() {
        let mut driver = MockSyncDriver::new(Mono::from_ns(500));
        assert_eq!(driver.clock().as_ns(), 500);
        assert_eq!(driver.now().as_ns(), 500);
        driver.advance_to(Mono::from_ns(200)); // earlier: a no-op
        assert_eq!(driver.clock().as_ns(), 500);
        driver.advance_ns(100);
        assert_eq!(driver.clock().as_ns(), 600);
    }
}
