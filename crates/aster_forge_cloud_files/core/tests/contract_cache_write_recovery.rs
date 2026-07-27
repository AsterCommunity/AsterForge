mod support;

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesStoreErrorKind, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
    CloudScope, ContentCacheKey, ContentCacheWriteExtent, ContentCacheWriteIntent,
    ContentCacheWriteOperationId, ContentCacheWriteState, ContentCacheWriteStore,
    ContentEvictionBegin, ContentEvictionBlocker, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionTarget, ContentRevision, ContentStorageEntry,
    ContentStorageMode, ContentStorageStore, SessionGeneration, StoreWriteStatus,
};

use support::{
    SyntheticBackend, content_storage::MemoryContentStorageStore, failure_injection::FailurePoint,
};

fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("range fixture should be valid")
}

fn revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes()).expect("revision fixture should be valid")
}

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn cache_key(backend: &SyntheticBackend, value: &str) -> ContentCacheKey {
    ContentCacheKey::new(backend.moved_file().key().clone(), revision(value))
}

fn scoped_cache_key(namespace: &str, root: &str, item: &str, value: &str) -> ContentCacheKey {
    ContentCacheKey::new(
        CloudItemKey::new(
            CloudScope::new(
                CloudNamespaceId::new(namespace).expect("namespace fixture should be valid"),
                CloudRootId::new(root).expect("root fixture should be valid"),
            ),
            CloudItemId::new(item).expect("item fixture should be valid"),
        ),
        revision(value),
    )
}

fn operation_id(value: &str) -> ContentCacheWriteOperationId {
    ContentCacheWriteOperationId::new(value).expect("cache write operation should be valid")
}

fn write_intent(
    operation: &str,
    key: ContentCacheKey,
    expected_size: u64,
    extent: ContentCacheWriteExtent,
) -> ContentCacheWriteIntent {
    ContentCacheWriteIntent::new(
        operation_id(operation),
        key,
        expected_size,
        extent,
        generation(1),
    )
    .expect("cache write intent should be valid")
}

async fn create_provider_entry(store: &MemoryContentStorageStore, key: ContentCacheKey, size: u64) {
    store
        .create_content_entry(ContentStorageEntry::new(
            key,
            ContentStorageMode::ProviderManaged,
            size,
        ))
        .await
        .expect("provider entry creation should succeed");
}

#[test]
fn cache_write_intent_rejects_ranges_beyond_exact_revision_size() {
    let backend = SyntheticBackend::full();
    let error = ContentCacheWriteIntent::new(
        operation_id("invalid-range"),
        cache_key(&backend, "cache-write-v1"),
        16,
        ContentCacheWriteExtent::Range(range(12, 8)),
        generation(1),
    )
    .expect_err("cache write range must remain within exact revision size");
    assert!(error.to_string().contains("expected content size"));
}

#[tokio::test]
async fn physical_bytes_must_be_observable_before_coverage_can_advance() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "ordered-write-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    let operation = operation_id("ordered-write");
    store
        .begin_content_cache_write(
            ContentCacheWriteIntent::new(
                operation.clone(),
                key.clone(),
                16,
                ContentCacheWriteExtent::Range(range(4, 8)),
                generation(1),
            )
            .expect("write intent should be valid"),
        )
        .await
        .expect("write intent should persist");

    let error = store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect_err("coverage must not claim bytes before physical commit");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
    assert!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .provider_ranges()
            .expect("provider ranges should exist")
            .ranges()
            .is_empty()
    );

    store
        .record_content_cache_write_physical_commit(&operation)
        .await
        .expect("physical commit marker should persist");
    store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect("coverage reconciliation should succeed");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .provider_ranges()
            .expect("provider ranges should exist")
            .ranges(),
        &[range(4, 8)]
    );
}

#[tokio::test]
async fn durable_cache_write_reservation_blocks_eviction_until_completion() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "write-eviction-race-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    let operation = operation_id("write-eviction-race");
    store
        .begin_content_cache_write(
            ContentCacheWriteIntent::new(
                operation.clone(),
                key.clone(),
                16,
                ContentCacheWriteExtent::Whole,
                generation(1),
            )
            .expect("write intent should be valid"),
        )
        .await
        .expect("write intent should persist");

    assert_eq!(
        store
            .begin_content_eviction(ContentEvictionIntent::new(
                ContentEvictionOperationId::new("blocked-eviction")
                    .expect("eviction operation should be valid"),
                key.clone(),
                ContentEvictionTarget::ProviderCache,
                generation(1),
            ))
            .await
            .expect("blocked eviction should return a decision"),
        ContentEvictionBegin::Blocked {
            blockers: vec![ContentEvictionBlocker::CacheWriteInProgress]
        }
    );

    store
        .record_content_cache_write_physical_commit(&operation)
        .await
        .expect("physical commit should succeed");
    store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect("coverage reconciliation should succeed");
    store
        .complete_content_cache_write(&operation)
        .await
        .expect("write completion should succeed");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .active_cache_writes(),
        0
    );
    assert!(matches!(
        store
            .begin_content_eviction(ContentEvictionIntent::new(
                ContentEvictionOperationId::new("allowed-eviction")
                    .expect("eviction operation should be valid"),
                key,
                ContentEvictionTarget::ProviderCache,
                generation(1),
            ))
            .await
            .expect("eviction should now reserve"),
        ContentEvictionBegin::Persisted { .. }
    ));
}

#[tokio::test]
async fn platform_managed_content_rejects_provider_cache_write_reservation() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "platform-write-v1");
    store
        .create_content_entry(ContentStorageEntry::new(
            key.clone(),
            ContentStorageMode::PlatformManaged,
            16,
        ))
        .await
        .expect("platform entry creation should succeed");
    let error = store
        .begin_content_cache_write(write_intent(
            "platform-write",
            key,
            16,
            ContentCacheWriteExtent::Whole,
        ))
        .await
        .expect_err("platform-managed bytes have no provider cache write target");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
}

#[tokio::test]
async fn physical_effect_before_marker_is_recovered_without_claiming_coverage_early() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "physical-recovery-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    let operation = operation_id("physical-recovery");
    store
        .begin_content_cache_write(
            ContentCacheWriteIntent::new(
                operation.clone(),
                key.clone(),
                16,
                ContentCacheWriteExtent::Range(range(0, 8)),
                generation(1),
            )
            .expect("write intent should be valid"),
        )
        .await
        .expect("write intent should persist");

    let physical_range_present = true;
    assert_eq!(
        store
            .load_content_cache_write(&operation)
            .await
            .expect("write load should succeed")
            .expect("write should exist")
            .state(),
        ContentCacheWriteState::IntentPersisted
    );
    assert!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .provider_ranges()
            .expect("provider ranges should exist")
            .ranges()
            .is_empty()
    );

    if physical_range_present {
        store
            .record_content_cache_write_physical_commit(&operation)
            .await
            .expect("startup observation should reconstruct physical marker");
    }
    store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect("recovered coverage should commit");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .provider_ranges()
            .expect("provider ranges should exist")
            .ranges(),
        &[range(0, 8)]
    );
}

#[tokio::test]
async fn every_cache_write_persisted_transition_survives_lost_return() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "lost-cache-write-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    let operation = operation_id("lost-cache-write");
    let intent = ContentCacheWriteIntent::new(
        operation.clone(),
        key,
        16,
        ContentCacheWriteExtent::Range(range(8, 8)),
        generation(1),
    )
    .expect("write intent should be valid");

    store
        .failures()
        .arm(FailurePoint::AfterContentCacheWriteIntentPersistBeforeReturn);
    store
        .begin_content_cache_write(intent.clone())
        .await
        .expect_err("intent return should be lost");
    assert_eq!(
        store
            .begin_content_cache_write(intent)
            .await
            .expect("intent retry should find durable state"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentCacheWritePhysicalPersistBeforeReturn);
    store
        .record_content_cache_write_physical_commit(&operation)
        .await
        .expect_err("physical marker return should be lost");
    assert_eq!(
        store
            .record_content_cache_write_physical_commit(&operation)
            .await
            .expect("physical marker retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentCacheWriteCoveragePersistBeforeReturn);
    store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect_err("coverage return should be lost");
    assert_eq!(
        store
            .reconcile_content_cache_write_coverage(&operation)
            .await
            .expect("coverage retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentCacheWriteCompletionPersistBeforeReturn);
    store
        .complete_content_cache_write(&operation)
        .await
        .expect_err("completion return should be lost");
    assert_eq!(
        store
            .complete_content_cache_write(&operation)
            .await
            .expect("completion retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert!(
        store
            .recoverable_content_cache_writes(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .is_empty()
    );
}

#[tokio::test]
async fn whole_empty_file_commit_is_complete_without_synthetic_nonempty_range() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "empty-cache-v1");
    create_provider_entry(&store, key.clone(), 0).await;
    let operation = operation_id("empty-cache");
    store
        .begin_content_cache_write(write_intent(
            "empty-cache",
            key.clone(),
            0,
            ContentCacheWriteExtent::Whole,
        ))
        .await
        .expect("empty write intent should persist");
    store
        .record_content_cache_write_physical_commit(&operation)
        .await
        .expect("empty physical commit should be observable");
    store
        .reconcile_content_cache_write_coverage(&operation)
        .await
        .expect("empty coverage reconciliation should succeed");
    let entry = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    let coverage = entry
        .provider_ranges()
        .expect("provider coverage should exist");
    assert!(coverage.ranges().is_empty());
    assert!(coverage.is_complete(0));
}

#[tokio::test]
async fn cache_write_store_rejects_missing_entry_size_mismatch_and_operation_conflict() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let missing_key = cache_key(&backend, "missing-entry-v1");
    let missing = write_intent(
        "missing-entry",
        missing_key,
        16,
        ContentCacheWriteExtent::Whole,
    );
    assert_eq!(
        store
            .begin_content_cache_write(missing)
            .await
            .expect_err("missing entry should fail")
            .kind(),
        CloudFilesStoreErrorKind::NotFound
    );

    let key = cache_key(&backend, "conflict-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    assert_eq!(
        store
            .begin_content_cache_write(write_intent(
                "wrong-size",
                key.clone(),
                15,
                ContentCacheWriteExtent::Whole,
            ))
            .await
            .expect_err("size mismatch should fail")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );

    let original = write_intent(
        "same-operation",
        key.clone(),
        16,
        ContentCacheWriteExtent::Range(range(0, 8)),
    );
    assert_eq!(
        store
            .begin_content_cache_write(original.clone())
            .await
            .expect("first intent should persist"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .begin_content_cache_write(original)
            .await
            .expect("same intent retry should succeed"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .begin_content_cache_write(write_intent(
                "same-operation",
                key,
                16,
                ContentCacheWriteExtent::Range(range(8, 8)),
            ))
            .await
            .expect_err("same operation with another extent should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
}

#[tokio::test]
async fn two_durable_cache_writes_keep_eviction_blocked_until_both_complete() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "two-writes-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    let first = operation_id("write-a");
    let second = operation_id("write-b");
    for (operation, extent) in [
        (first.clone(), ContentCacheWriteExtent::Range(range(0, 8))),
        (second.clone(), ContentCacheWriteExtent::Range(range(8, 8))),
    ] {
        store
            .begin_content_cache_write(
                ContentCacheWriteIntent::new(operation, key.clone(), 16, extent, generation(1))
                    .expect("intent fixture should be valid"),
            )
            .await
            .expect("write intent should persist");
    }
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .active_cache_writes(),
        2
    );

    for operation in [&first, &second] {
        store
            .record_content_cache_write_physical_commit(operation)
            .await
            .expect("physical commit should persist");
        store
            .reconcile_content_cache_write_coverage(operation)
            .await
            .expect("coverage should reconcile");
    }
    store
        .complete_content_cache_write(&first)
        .await
        .expect("first write should complete");
    assert_eq!(
        store
            .begin_content_eviction(ContentEvictionIntent::new(
                ContentEvictionOperationId::new("still-blocked")
                    .expect("operation fixture should be valid"),
                key.clone(),
                ContentEvictionTarget::ProviderCache,
                generation(1),
            ))
            .await
            .expect("eviction decision should succeed"),
        ContentEvictionBegin::Blocked {
            blockers: vec![ContentEvictionBlocker::CacheWriteInProgress]
        }
    );
    store
        .complete_content_cache_write(&second)
        .await
        .expect("second write should complete");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .active_cache_writes(),
        0
    );
}

#[tokio::test]
async fn cache_write_recovery_is_scope_filtered_sorted_and_excludes_completed_records() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "recovery-v1");
    create_provider_entry(&store, key.clone(), 16).await;
    for operation in ["z-write", "a-write"] {
        store
            .begin_content_cache_write(write_intent(
                operation,
                key.clone(),
                16,
                ContentCacheWriteExtent::Whole,
            ))
            .await
            .expect("write intent should persist");
    }
    let other_key = scoped_cache_key("other-namespace", "other-root", "other-file", "other-v1");
    create_provider_entry(&store, other_key.clone(), 16).await;
    store
        .begin_content_cache_write(write_intent(
            "m-other-write",
            other_key.clone(),
            16,
            ContentCacheWriteExtent::Whole,
        ))
        .await
        .expect("other-scope write intent should persist");
    let records = store
        .recoverable_content_cache_writes(backend.scope())
        .await
        .expect("recovery scan should succeed");
    assert_eq!(
        records
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["a-write", "z-write"]
    );

    let completed = operation_id("a-write");
    store
        .record_content_cache_write_physical_commit(&completed)
        .await
        .expect("physical marker should persist");
    store
        .reconcile_content_cache_write_coverage(&completed)
        .await
        .expect("coverage should reconcile");
    store
        .complete_content_cache_write(&completed)
        .await
        .expect("write should complete");
    assert_eq!(
        store
            .recoverable_content_cache_writes(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["z-write"]
    );
    assert_eq!(
        store
            .recoverable_content_cache_writes(other_key.item_key().scope())
            .await
            .expect("other-scope recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["m-other-write"]
    );
}
