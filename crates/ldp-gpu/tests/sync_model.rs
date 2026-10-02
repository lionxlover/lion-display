//! The sync model at integration level: fences across provenances,
//! the AND-merge contract, and the interlock with `ldp-display`'s mock
//! out-fence tokens (mapped by value, since the crates share no
//! dependency edge by design).

use std::os::fd::{AsRawFd, OwnedFd};

use ldp_core::time::Mono;
use ldp_gpu::sync::{Fence, FenceKind, FenceState, MockSyncDriver, SyncDriver, TimelinePoint};

#[test]
fn mixed_provenance_merge_waits_for_every_member() {
    let mut driver = MockSyncDriver::new(Mono::ZERO);
    // One member of each provenance: a mock token (ldp-display's mock
    // out-fence), a timeline point, and a real sync-file descriptor
    // backed by /dev/null so the Fence genuinely owns it.
    let file = owned_null_fd();
    let file_number = file.as_raw_fd();
    driver.schedule_token(9, Mono::from_ns(5_000_000));
    driver.signal_timeline(2, 100);
    driver.schedule_sync_file(file_number, Mono::from_ns(9_000_000));

    let merged = Fence::merge(vec![
        Fence::Mock(9),
        Fence::Timeline(TimelinePoint {
            timeline: 2,
            point: 100,
        }),
        Fence::SyncFile(file),
    ]);
    assert_eq!(merged.kind(), FenceKind::All(3));

    // The timeline point is already signaled; the token matures at
    // 5 ms; the sync file at 9 ms — the AND-merge follows the latest.
    driver.advance_ns(5_000_000);
    assert!(!driver.state(&merged).unwrap().is_signaled());
    driver.advance_ns(4_000_000);
    assert!(driver.state(&merged).expect("tracked fence").is_signaled());
    assert_eq!(driver.state(&merged).unwrap(), FenceState::Signaled);
}

#[test]
fn fence_drivers_report_their_clock() {
    let mut driver = MockSyncDriver::new(Mono::from_ns(1_234));
    assert_eq!(driver.now().as_ns(), 1_234);
    driver.advance_ns(1_000);
    assert_eq!(SyncDriver::now(&driver).as_ns(), 2_234);
}

#[test]
fn unknown_fences_fail_loudly_not_silently() {
    let driver = MockSyncDriver::new(Mono::ZERO);
    assert!(driver.state(&Fence::Mock(404)).is_err());
    assert!(driver
        .state(&Fence::Timeline(TimelinePoint {
            timeline: 7,
            point: 1
        }))
        .is_err());
}

#[test]
fn clone_preserves_shape_and_state() {
    let mut driver = MockSyncDriver::new(Mono::ZERO);
    driver.schedule_token(5, Mono::from_ns(1_000));
    let original = Fence::Mock(5);
    let cloned = original.clone();
    assert_eq!(original, cloned);
    driver.advance_ns(1_000);
    assert_eq!(
        driver.state(&original).unwrap(),
        driver.state(&cloned).unwrap()
    );
    // Timeline and merge clones keep their shapes.
    let merged = Fence::merge(vec![Fence::Mock(5), Fence::Mock(5)]);
    assert_eq!(merged.clone(), merged);
}

#[test]
fn sync_file_fences_track_their_descriptor() {
    let mut driver = MockSyncDriver::new(Mono::ZERO);
    let file = owned_null_fd();
    let number = file.as_raw_fd();
    driver.schedule_sync_file(number, Mono::from_ns(2_000));
    let fence = Fence::SyncFile(file);
    assert!(!driver.state(&fence).unwrap().is_signaled());
    driver.advance_ns(2_000);
    assert!(driver.state(&fence).unwrap().is_signaled());
}

/// A genuinely owned descriptor backed by `/dev/null` — the sync-file
/// provenance with a real fd lifecycle.
fn owned_null_fd() -> OwnedFd {
    let file = std::fs::File::open("/dev/null").expect("/dev/null exists in this sandbox");
    OwnedFd::from(file)
}
