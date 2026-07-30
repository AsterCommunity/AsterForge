mod support;

use aster_forge_cloud_files_core::{
    ByteRange, ChangeBatchId, ChangeCheckpointStore, CloudFilesStoreErrorKind, CloudItemId,
    CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope, ContentCacheKey,
    ContentCacheWriteOperationId, ContentCacheWriteStore, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionPhysicalEffect, ContentEvictionTarget,
    ContentLeaseId, ContentLeaseKind, ContentRevision, ContentStorageStore, ContentUploadSession,
    ContentUploadSessionId, ContentUploadStore, LocalContentGeneration, LocalContentReference,
    LocalContentSnapshot, MutationJournalStore, MutationRemoteOutcome, OperationId,
    PlatformMaterializationState, SessionGeneration, SessionState, StoreResult, StoreWriteStatus,
};

use support::{content_storage::MemoryContentStorageStore, store::MemoryCloudFilesStore};

fn scope(namespace: &str, root: &str) -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(namespace).expect("namespace fixture should be valid"),
        CloudRootId::new(root).expect("root fixture should be valid"),
    )
}

fn cache_key() -> ContentCacheKey {
    ContentCacheKey::new(
        CloudItemKey::new(
            scope("namespace", "root"),
            CloudItemId::new("file").expect("item fixture should be valid"),
        ),
        ContentRevision::from_slice(b"content-v1").expect("revision fixture should be valid"),
    )
}

fn assert_store_error_kind<T: std::fmt::Debug>(
    result: StoreResult<T>,
    expected: CloudFilesStoreErrorKind,
) {
    let error = result.expect_err("store operation should fail");
    assert_eq!(error.kind(), expected, "{}", error.context());
}

#[tokio::test]
async fn missing_change_checkpoint_transitions_are_not_found() {
    let store = MemoryCloudFilesStore::default();
    let scope = scope("namespace", "root");
    let batch_id = ChangeBatchId::new("missing").expect("batch id fixture should be valid");
    assert_store_error_kind(
        store
            .mark_change_effects_replayable(&scope, &batch_id)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.commit_change_cursor(&scope, &batch_id).await,
        CloudFilesStoreErrorKind::NotFound,
    );
}

#[tokio::test]
async fn session_store_covers_absent_stale_newer_invalid_and_closed_boundaries() {
    let store = MemoryCloudFilesStore::default();
    let scope = scope("namespace", "root");
    let one = SessionGeneration::new(1).expect("generation fixture should be valid");
    let two = SessionGeneration::new(2).expect("generation fixture should be valid");
    let three = SessionGeneration::new(3).expect("generation fixture should be valid");

    assert_eq!(
        store
            .load_session(&scope)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store
            .transition_session(&scope, one, SessionState::Closing)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_eq!(
        store
            .activate_session(&scope, two)
            .await
            .expect("activation should succeed"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .activate_session(&scope, one)
            .await
            .expect("stale activation should fence"),
        StoreWriteStatus::Fenced
    );
    assert_eq!(
        store
            .activate_session(&scope, two)
            .await
            .expect("duplicate activation should succeed"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_store_error_kind(
        store
            .transition_session(&scope, three, SessionState::Closing)
            .await,
        CloudFilesStoreErrorKind::Conflict,
    );
    assert_eq!(
        store
            .transition_session(&scope, one, SessionState::Closing)
            .await
            .expect("stale transition should fence"),
        StoreWriteStatus::Fenced
    );
    assert_store_error_kind(
        store
            .transition_session(&scope, two, SessionState::Draining)
            .await,
        CloudFilesStoreErrorKind::InvalidTransition,
    );
    assert_eq!(
        store
            .transition_session(&scope, two, SessionState::Closing)
            .await
            .expect("closing should succeed"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .transition_session(&scope, two, SessionState::Closing)
            .await
            .expect("duplicate closing should succeed"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_store_error_kind(
        store.activate_session(&scope, two).await,
        CloudFilesStoreErrorKind::InvalidTransition,
    );
    assert_eq!(
        store
            .activate_session(&scope, three)
            .await
            .expect("new generation should activate"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn every_missing_mutation_transition_is_not_found() {
    let store = MemoryCloudFilesStore::default();
    let operation = OperationId::new("missing").expect("operation fixture should be valid");
    let generation = SessionGeneration::new(1).expect("generation fixture should be valid");
    assert_eq!(
        store
            .load_mutation(&operation)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store.begin_remote_apply(&operation, generation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .record_remote_outcome(
                &operation,
                generation,
                MutationRemoteOutcome::RemoteOutcomeUnknown,
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.mark_platform_reconciled(&operation, generation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.complete_mutation(&operation, generation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
}

#[tokio::test]
async fn every_missing_content_entry_mutation_is_not_found() {
    let store = MemoryContentStorageStore::default();
    let key = cache_key();
    let lease = ContentLeaseId::new("lease").expect("lease fixture should be valid");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store
            .record_provider_cached_range(
                &key,
                ByteRange::new(0, 1).expect("range should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .observe_platform_materialization(&key, PlatformMaterializationState::Complete)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.set_content_pinned(&key, true).await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .mark_content_dirty(
                &key,
                LocalContentSnapshot::new(
                    key.item_key().clone(),
                    LocalContentGeneration::new(1).expect("generation should be valid"),
                    LocalContentReference::new("local://missing")
                        .expect("reference fixture should be valid"),
                    1,
                    None,
                ),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .clear_content_dirty(
                &key,
                LocalContentGeneration::new(1).expect("generation should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .acquire_content_lease(&key, lease.clone(), ContentLeaseKind::Read)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.release_content_lease(&key, &lease).await,
        CloudFilesStoreErrorKind::NotFound,
    );
    let intent = ContentEvictionIntent::new(
        ContentEvictionOperationId::new("eviction").expect("operation fixture should be valid"),
        key,
        ContentEvictionTarget::ProviderCache,
        SessionGeneration::new(1).expect("generation should be valid"),
    );
    assert_store_error_kind(
        store.begin_content_eviction(intent).await,
        CloudFilesStoreErrorKind::NotFound,
    );
}

#[tokio::test]
async fn every_missing_cache_write_transition_is_not_found() {
    let store = MemoryContentStorageStore::default();
    let operation =
        ContentCacheWriteOperationId::new("missing").expect("operation fixture should be valid");
    assert_eq!(
        store
            .load_content_cache_write(&operation)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store
            .record_content_cache_write_physical_commit(&operation)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .reconcile_content_cache_write_coverage(&operation)
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.complete_content_cache_write(&operation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
}

#[tokio::test]
async fn every_missing_upload_transition_is_not_found() {
    let store = MemoryContentStorageStore::default();
    let operation = OperationId::new("missing").expect("operation fixture should be valid");
    assert_eq!(
        store
            .load_content_upload(&operation)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store
            .record_content_upload_session(
                &operation,
                ContentUploadSession::new(
                    ContentUploadSessionId::new("session")
                        .expect("session fixture should be valid"),
                    0,
                ),
                SessionGeneration::new(1).expect("generation fixture should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .begin_content_upload_remote_commit(
                &operation,
                SessionGeneration::new(1).expect("generation fixture should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .record_content_upload_remote_outcome(
                &operation,
                MutationRemoteOutcome::RemoteOutcomeUnknown,
                SessionGeneration::new(1).expect("generation fixture should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .reconcile_content_upload_metadata(
                &operation,
                SessionGeneration::new(1).expect("generation fixture should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store
            .complete_content_upload(
                &operation,
                SessionGeneration::new(1).expect("generation fixture should be valid"),
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
}

#[tokio::test]
async fn every_missing_eviction_transition_is_not_found() {
    let store = MemoryContentStorageStore::default();
    let operation =
        ContentEvictionOperationId::new("missing").expect("operation fixture should be valid");
    assert_eq!(
        store
            .load_content_eviction(&operation)
            .await
            .expect("load should succeed"),
        None
    );
    assert_store_error_kind(
        store
            .record_content_eviction_effect(
                &operation,
                ContentEvictionPhysicalEffect::ProviderCacheRemoved,
            )
            .await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.reconcile_content_eviction_metadata(&operation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
    assert_store_error_kind(
        store.complete_content_eviction(&operation).await,
        CloudFilesStoreErrorKind::NotFound,
    );
}
