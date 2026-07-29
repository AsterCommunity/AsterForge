mod support;

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesCoreError, CloudFilesStoreErrorKind, ContentCacheKey,
    ContentEvictionBegin, ContentEvictionBlocker, ContentEvictionDecision, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionTarget, ContentLeaseId, ContentLeaseKind,
    ContentRangeSet, ContentRevision, ContentStorageEntry, ContentStorageMode, ContentStorageStore,
    LocalContentGeneration, LocalContentReference, LocalContentSnapshot,
    PlatformMaterializationState, SessionGeneration, StoreWriteStatus,
};

use support::{SyntheticBackend, content_storage::MemoryContentStorageStore};

fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("range fixture should be valid")
}

fn revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes()).expect("revision fixture should be valid")
}

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn operation_id(value: &str) -> ContentEvictionOperationId {
    ContentEvictionOperationId::new(value).expect("eviction operation fixture should be valid")
}

fn lease_id(value: &str) -> ContentLeaseId {
    ContentLeaseId::new(value).expect("lease fixture should be valid")
}

fn local_generation(value: u64) -> LocalContentGeneration {
    LocalContentGeneration::new(value).expect("local generation fixture should be valid")
}

fn local_snapshot(
    backend: &SyntheticBackend,
    generation: u64,
    reference: &str,
) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        backend.moved_file().key().clone(),
        local_generation(generation),
        LocalContentReference::new(reference).expect("local reference fixture should be valid"),
        16,
        None,
    )
}

fn cache_key(backend: &SyntheticBackend, revision: &str) -> ContentCacheKey {
    ContentCacheKey::new(backend.moved_file().key().clone(), self::revision(revision))
}

fn eviction_intent(
    key: ContentCacheKey,
    operation: &str,
    target: ContentEvictionTarget,
) -> ContentEvictionIntent {
    ContentEvictionIntent::new(operation_id(operation), key, target, generation(1))
}

#[test]
fn cache_identity_survives_rename_and_separates_content_revisions() {
    let backend = SyntheticBackend::full();
    assert_eq!(
        backend.moved_file_before().key(),
        backend.moved_file().key(),
        "rename and same-root move must preserve the stable item key"
    );

    let before = ContentCacheKey::new(
        backend.moved_file_before().key().clone(),
        revision("content-v1"),
    );
    let after_same_revision =
        ContentCacheKey::new(backend.moved_file().key().clone(), revision("content-v1"));
    let after_new_revision =
        ContentCacheKey::new(backend.moved_file().key().clone(), revision("content-v2"));

    assert_eq!(before, after_same_revision);
    assert_ne!(before, after_new_revision);
}

#[test]
fn sparse_ranges_merge_overlap_and_adjacency_without_exceeding_size() {
    let mut ranges = ContentRangeSet::new();
    assert!(ranges.insert(range(8, 4), 20).expect("range should fit"));
    assert!(ranges.insert(range(0, 4), 20).expect("range should fit"));
    assert!(
        ranges
            .insert(range(4, 4), 20)
            .expect("adjacent range should merge")
    );
    assert!(
        ranges
            .insert(range(10, 6), 20)
            .expect("overlapping range should merge")
    );
    assert_eq!(ranges.ranges(), &[range(0, 16)]);
    assert!(ranges.covers(range(3, 9)));
    assert!(!ranges.is_complete(20));
    assert!(
        ranges
            .insert(range(16, 4), 20)
            .expect("tail range should complete coverage")
    );
    assert_eq!(ranges.ranges(), &[range(0, 20)]);
    assert!(ranges.is_complete(20));
    assert!(ContentRangeSet::new().is_complete(0));

    let error = ranges
        .insert(range(20, 1), 20)
        .expect_err("coverage beyond logical size must be rejected");
    assert!(matches!(
        error,
        CloudFilesCoreError::InvalidContentStorageState { .. }
    ));
}

#[test]
fn ownership_modes_keep_provider_coverage_and_platform_state_separate() {
    let backend = SyntheticBackend::full();
    let mut platform = ContentStorageEntry::new(
        cache_key(&backend, "platform-v1"),
        ContentStorageMode::PlatformManaged,
        16,
    );
    assert!(platform.provider_ranges().is_none());
    assert_eq!(
        platform.platform_materialization(),
        Some(PlatformMaterializationState::Unknown)
    );
    platform
        .observe_platform_materialization(PlatformMaterializationState::Complete)
        .expect("platform observation should be accepted");
    assert!(platform.record_provider_range(range(0, 4)).is_err());
    assert!(platform.provider_ranges().is_none());

    let mut provider = ContentStorageEntry::new(
        cache_key(&backend, "provider-v1"),
        ContentStorageMode::ProviderManaged,
        16,
    );
    provider
        .record_provider_range(range(4, 4))
        .expect("provider range should be accepted");
    assert_eq!(
        provider
            .provider_ranges()
            .expect("provider coverage should exist")
            .ranges(),
        &[range(4, 4)]
    );
    assert!(provider.platform_materialization().is_none());
    assert!(
        provider
            .observe_platform_materialization(PlatformMaterializationState::Partial)
            .is_err()
    );

    let mut hybrid = ContentStorageEntry::new(
        cache_key(&backend, "hybrid-v1"),
        ContentStorageMode::Hybrid,
        16,
    );
    hybrid
        .record_provider_range(range(0, 8))
        .expect("hybrid provider range should be accepted");
    hybrid
        .observe_platform_materialization(PlatformMaterializationState::Partial)
        .expect("hybrid platform observation should be accepted");
    assert_eq!(
        hybrid
            .provider_ranges()
            .expect("hybrid provider coverage should exist")
            .ranges(),
        &[range(0, 8)]
    );
    assert_eq!(
        hybrid.platform_materialization(),
        Some(PlatformMaterializationState::Partial)
    );
}

#[test]
fn eviction_blockers_are_complete_and_deterministically_ordered() {
    let backend = SyntheticBackend::full();
    let mut entry = ContentStorageEntry::new(
        cache_key(&backend, "guard-v1"),
        ContentStorageMode::Hybrid,
        16,
    );
    entry.set_pinned(true).expect("pin should succeed");
    entry
        .mark_dirty(local_snapshot(&backend, 1, "dirty-1"))
        .expect("dirty snapshot should be recorded");
    for (index, kind) in [
        ContentLeaseKind::Open,
        ContentLeaseKind::Read,
        ContentLeaseKind::Write,
        ContentLeaseKind::Hydration,
        ContentLeaseKind::Upload,
        ContentLeaseKind::Verification,
    ]
    .into_iter()
    .enumerate()
    {
        entry
            .acquire_lease(lease_id(&format!("lease-{index}")), kind)
            .expect("guard lease should be acquired");
    }

    assert_eq!(
        entry.eviction_blockers(),
        vec![
            ContentEvictionBlocker::Pinned,
            ContentEvictionBlocker::Dirty,
            ContentEvictionBlocker::OpenLease,
            ContentEvictionBlocker::ReadLease,
            ContentEvictionBlocker::WriteLease,
            ContentEvictionBlocker::HydrationLease,
            ContentEvictionBlocker::UploadLease,
            ContentEvictionBlocker::VerificationLease,
        ]
    );
    assert!(matches!(
        entry
            .reserve_eviction(ContentEvictionTarget::AllManagedContent)
            .expect("target should be valid"),
        ContentEvictionDecision::Blocked { blockers }
            if blockers.len() == 8
    ));
}

#[tokio::test]
async fn hydration_lease_blocks_eviction_until_release_then_reservation_fences_new_leases() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "race-v1");
    store
        .create_content_entry(ContentStorageEntry::new(
            key.clone(),
            ContentStorageMode::ProviderManaged,
            16,
        ))
        .await
        .expect("entry creation should succeed");
    let hydration = lease_id("hydration-1");
    store
        .acquire_content_lease(&key, hydration.clone(), ContentLeaseKind::Hydration)
        .await
        .expect("hydration lease should be acquired");

    let blocked = store
        .begin_content_eviction(eviction_intent(
            key.clone(),
            "eviction-blocked",
            ContentEvictionTarget::ProviderCache,
        ))
        .await
        .expect("blocked decision should be returned");
    assert_eq!(
        blocked,
        ContentEvictionBegin::Blocked {
            blockers: vec![ContentEvictionBlocker::HydrationLease]
        }
    );
    store
        .release_content_lease(&key, &hydration)
        .await
        .expect("hydration lease release should succeed");

    assert!(matches!(
        store
            .begin_content_eviction(eviction_intent(
                key.clone(),
                "eviction-reserved",
                ContentEvictionTarget::ProviderCache,
            ))
            .await
            .expect("eviction reservation should succeed"),
        ContentEvictionBegin::Persisted { .. }
    ));
    let error = store
        .acquire_content_lease(&key, lease_id("late-reader"), ContentLeaseKind::Read)
        .await
        .expect_err("new lease must not enter after durable eviction reservation");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
}

#[tokio::test]
async fn content_entry_creation_and_runtime_updates_are_idempotent_but_conflicts_are_visible() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "entry-v1");
    let entry = ContentStorageEntry::new(key.clone(), ContentStorageMode::Hybrid, 16);
    assert_eq!(
        store
            .create_content_entry(entry.clone())
            .await
            .expect("first create should succeed"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .create_content_entry(entry)
            .await
            .expect("same create should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    let conflict = store
        .create_content_entry(ContentStorageEntry::new(
            key.clone(),
            ContentStorageMode::Hybrid,
            17,
        ))
        .await
        .expect_err("same cache key must not replace a different snapshot");
    assert_eq!(conflict.kind(), CloudFilesStoreErrorKind::Conflict);

    assert_eq!(
        store
            .record_provider_cached_range(&key, range(0, 8))
            .await
            .expect("range write should succeed"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .record_provider_cached_range(&key, range(2, 2))
            .await
            .expect("covered range write should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .observe_platform_materialization(&key, PlatformMaterializationState::Partial)
            .await
            .expect("platform observation should succeed"),
        StoreWriteStatus::Applied
    );
    let snapshot = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert_eq!(
        snapshot
            .provider_ranges()
            .expect("hybrid provider coverage should exist")
            .ranges(),
        &[range(0, 8)]
    );
    assert_eq!(
        snapshot.platform_materialization(),
        Some(PlatformMaterializationState::Partial)
    );
}

#[test]
fn dirty_snapshots_advance_monotonically_and_fence_stale_generations() {
    let backend = SyntheticBackend::full();
    let mut entry = ContentStorageEntry::new(
        cache_key(&backend, "dirty-generation-v1"),
        ContentStorageMode::ProviderManaged,
        16,
    );
    let first = local_snapshot(&backend, 1, "dirty-generation-1");
    let second = local_snapshot(&backend, 2, "dirty-generation-2");
    assert_eq!(
        entry
            .mark_dirty(first.clone())
            .expect("first dirty snapshot should apply"),
        aster_forge_cloud_files_core::DirtyContentTransition::Applied
    );
    assert_eq!(
        entry
            .mark_dirty(second.clone())
            .expect("newer dirty snapshot should apply"),
        aster_forge_cloud_files_core::DirtyContentTransition::Applied
    );
    assert_eq!(
        entry
            .mark_dirty(first)
            .expect("older dirty snapshot should be fenced"),
        aster_forge_cloud_files_core::DirtyContentTransition::Fenced
    );
    assert_eq!(
        entry
            .clear_dirty(local_generation(1))
            .expect("older upload completion should be fenced"),
        aster_forge_cloud_files_core::DirtyContentTransition::Fenced
    );
    assert_eq!(entry.dirty_snapshot(), Some(&second));
}
