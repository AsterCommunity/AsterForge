mod support;

use aster_forge_cloud_files_core::{
    ChangeBatch, ChangeBatchId, ChangeCheckpointStore, ChangeCursor, ChangeCursorCheckpoint,
    CloudChange, CloudFilesCoreError, CloudFilesStoreError, CloudFilesStoreErrorKind, CloudItem,
    PersistedChangeBatch, PersistedChangeBatchState, StoreWriteStatus,
};

use support::{SyntheticBackend, failure_injection::FailurePoint, store::MemoryCloudFilesStore};

fn first_batch(backend: &SyntheticBackend, id: &str) -> (ChangeBatchId, PersistedChangeBatch) {
    let batch_id = ChangeBatchId::new(id).expect("batch id fixture should be valid");
    let batch = ChangeBatch::new(
        vec![CloudChange::Upsert {
            item: backend.moved_file().clone(),
        }],
        ChangeCursor::new(format!("cursor-{id}").into_bytes())
            .expect("cursor fixture should be valid"),
        false,
    );
    (
        batch_id.clone(),
        PersistedChangeBatch::new(batch_id, backend.scope().clone(), None, batch),
    )
}

#[tokio::test]
async fn persisted_batch_does_not_advance_the_active_cursor() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (_, batch) = first_batch(&backend, "one");

    store
        .record_change_batch(batch.clone())
        .await
        .expect("batch persistence should succeed");

    let checkpoint = store
        .load_change_checkpoint(backend.scope())
        .await
        .expect("checkpoint load should succeed");
    assert!(checkpoint.active_cursor().is_none());
    assert_eq!(checkpoint.pending_batch(), Some(&batch));
    assert_eq!(
        checkpoint
            .pending_batch()
            .expect("pending batch should remain durable")
            .state(),
        PersistedChangeBatchState::Recorded
    );
}

#[tokio::test]
async fn crash_after_batch_persist_keeps_old_cursor_and_replayable_input() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (_, batch) = first_batch(&backend, "crash-after-record");
    store
        .failures()
        .arm(FailurePoint::AfterChangeBatchPersistBeforeReturn);

    let error = store
        .record_change_batch(batch.clone())
        .await
        .expect_err("injected post-persist crash should surface");
    assert_eq!(
        error.kind(),
        aster_forge_cloud_files_core::CloudFilesStoreErrorKind::PersistenceFailure
    );

    let recovered = store
        .load_change_checkpoint(backend.scope())
        .await
        .expect("checkpoint recovery should succeed");
    assert!(recovered.active_cursor().is_none());
    assert_eq!(recovered.pending_batch(), Some(&batch));
}

#[tokio::test]
async fn cursor_commit_requires_replayable_effects() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (batch_id, batch) = first_batch(&backend, "not-replayable");
    store
        .record_change_batch(batch)
        .await
        .expect("batch persistence should succeed");

    let error = store
        .commit_change_cursor(backend.scope(), &batch_id)
        .await
        .expect_err("cursor must not advance early");
    assert_eq!(
        error.kind(),
        aster_forge_cloud_files_core::CloudFilesStoreErrorKind::InvalidTransition
    );
    assert!(
        store
            .load_change_checkpoint(backend.scope())
            .await
            .expect("checkpoint load should succeed")
            .active_cursor()
            .is_none()
    );
}

#[tokio::test]
async fn replayable_marker_survives_crash_and_allows_cursor_commit() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (batch_id, batch) = first_batch(&backend, "replayable");
    let expected_cursor = batch.batch().next_cursor().clone();
    store
        .record_change_batch(batch)
        .await
        .expect("batch persistence should succeed");
    store
        .failures()
        .arm(FailurePoint::AfterChangeEffectsReplayablePersistBeforeReturn);

    store
        .mark_change_effects_replayable(backend.scope(), &batch_id)
        .await
        .expect_err("injected post-marker crash should surface");

    let recovered = store
        .load_change_checkpoint(backend.scope())
        .await
        .expect("checkpoint recovery should succeed");
    assert_eq!(
        recovered
            .pending_batch()
            .expect("pending batch should remain")
            .state(),
        PersistedChangeBatchState::EffectsReplayable
    );

    assert_eq!(
        store
            .commit_change_cursor(backend.scope(), &batch_id)
            .await
            .expect("cursor commit should succeed"),
        StoreWriteStatus::Applied
    );
    let committed = store
        .load_change_checkpoint(backend.scope())
        .await
        .expect("checkpoint load should succeed");
    assert_eq!(committed.active_cursor(), Some(&expected_cursor));
    assert!(committed.pending_batch().is_none());
}

#[tokio::test]
async fn cursor_commit_is_idempotent_even_if_the_first_return_is_lost() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (batch_id, batch) = first_batch(&backend, "commit-once");
    let expected_cursor = batch.batch().next_cursor().clone();
    store
        .record_change_batch(batch)
        .await
        .expect("batch persistence should succeed");
    store
        .mark_change_effects_replayable(backend.scope(), &batch_id)
        .await
        .expect("replayable marker should succeed");
    store
        .failures()
        .arm(FailurePoint::AfterCursorCommitPersistBeforeReturn);

    store
        .commit_change_cursor(backend.scope(), &batch_id)
        .await
        .expect_err("first committed response is intentionally lost");

    assert_eq!(
        store
            .commit_change_cursor(backend.scope(), &batch_id)
            .await
            .expect("duplicate commit should be recognized"),
        StoreWriteStatus::AlreadyApplied
    );
    let checkpoint = store
        .load_change_checkpoint(backend.scope())
        .await
        .expect("checkpoint load should succeed");
    assert_eq!(checkpoint.active_cursor(), Some(&expected_cursor));
    assert!(checkpoint.pending_batch().is_none());
}

#[tokio::test]
async fn checkpoint_store_rejects_wrong_pending_id_second_pending_batch_and_base_cursor_drift() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (first_id, first) = first_batch(&backend, "first");
    store
        .record_change_batch(first.clone())
        .await
        .expect("first batch should persist");

    let wrong_id = ChangeBatchId::new("wrong").expect("batch id fixture should be valid");
    assert_eq!(
        store
            .mark_change_effects_replayable(backend.scope(), &wrong_id)
            .await
            .expect_err("wrong pending identity should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
    assert_eq!(
        store
            .commit_change_cursor(backend.scope(), &wrong_id)
            .await
            .expect_err("wrong pending identity should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );

    let (_, second) = first_batch(&backend, "second");
    assert_eq!(
        store
            .record_change_batch(second)
            .await
            .expect_err("second pending batch should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );

    store
        .mark_change_effects_replayable(backend.scope(), &first_id)
        .await
        .expect("first batch should become replayable");
    store
        .commit_change_cursor(backend.scope(), &first_id)
        .await
        .expect("first cursor should commit");

    let (_, stale_base) = first_batch(&backend, "stale-base");
    assert_eq!(
        store
            .record_change_batch(stale_base)
            .await
            .expect_err("batch fetched from stale base should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
    assert_eq!(
        store
            .mark_change_effects_replayable(backend.scope(), &wrong_id)
            .await
            .expect_err("missing pending batch should be not found")
            .kind(),
        CloudFilesStoreErrorKind::NotFound
    );
}

#[tokio::test]
async fn committed_batch_identity_cannot_be_reused_for_different_content() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let (batch_id, batch) = first_batch(&backend, "committed-id");
    store
        .record_change_batch(batch)
        .await
        .expect("batch should persist");
    store
        .mark_change_effects_replayable(backend.scope(), &batch_id)
        .await
        .expect("batch should become replayable");
    store
        .commit_change_cursor(backend.scope(), &batch_id)
        .await
        .expect("cursor should commit");

    let different = PersistedChangeBatch::new(
        batch_id,
        backend.scope().clone(),
        Some(
            ChangeCursor::from_slice(b"cursor-committed-id")
                .expect("base cursor fixture should be valid"),
        ),
        ChangeBatch::new(
            Vec::new(),
            ChangeCursor::from_slice(b"different-next").expect("cursor fixture should be valid"),
            false,
        ),
    );
    assert_eq!(
        store
            .record_change_batch(different)
            .await
            .expect_err("committed identity reuse should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
}

#[test]
fn persisted_change_batch_keeps_full_change_payload_for_replay() {
    let backend = SyntheticBackend::full();
    let (_, batch) = first_batch(&backend, "payload");
    let [CloudChange::Upsert { item }] = batch.batch().changes() else {
        panic!("fixture should contain one upsert");
    };
    let expected: &CloudItem = backend.moved_file();
    assert_eq!(item, expected);
}

#[test]
fn store_errors_and_batch_ids_preserve_classification_context_and_debug_shape() {
    let kinds = [
        CloudFilesStoreErrorKind::NotFound,
        CloudFilesStoreErrorKind::Conflict,
        CloudFilesStoreErrorKind::InvalidTransition,
        CloudFilesStoreErrorKind::PersistenceFailure,
    ];
    for kind in kinds {
        let error = CloudFilesStoreError::new(kind, "adapter context");
        assert_eq!(error.kind(), kind);
        assert_eq!(error.context(), "adapter context");
        assert_eq!(
            error.to_string(),
            format!("cloud-files store {kind:?}: adapter context")
        );
    }

    assert_eq!(
        ChangeBatchId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "change batch id",
        })
    );
    let id = ChangeBatchId::new(" batch ").expect("batch id fixture should be valid");
    assert_eq!(id.as_str(), " batch ");
    assert_eq!(format!("{id:?}"), "ChangeBatchId { byte_len: 7 }");
}

#[test]
fn persisted_batch_and_checkpoint_accessors_cover_initial_pending_and_replayable_states() {
    let backend = SyntheticBackend::full();
    let (_, mut batch) = first_batch(&backend, "accessors");
    assert_eq!(batch.id().as_str(), "accessors");
    assert_eq!(batch.scope(), backend.scope());
    assert!(batch.base_cursor().is_none());
    assert_eq!(batch.state(), PersistedChangeBatchState::Recorded);
    assert!(batch.mark_effects_replayable());
    assert!(!batch.mark_effects_replayable());
    assert_eq!(batch.state(), PersistedChangeBatchState::EffectsReplayable);

    let initial = ChangeCursorCheckpoint::initial(backend.scope().clone());
    assert_eq!(initial.scope(), backend.scope());
    assert!(initial.active_cursor().is_none());
    assert!(initial.pending_batch().is_none());

    let active = ChangeCursor::from_slice(b"active").expect("cursor fixture should be valid");
    let checkpoint = ChangeCursorCheckpoint::new(
        backend.scope().clone(),
        Some(active.clone()),
        Some(batch.clone()),
    );
    assert_eq!(checkpoint.scope(), backend.scope());
    assert_eq!(checkpoint.active_cursor(), Some(&active));
    assert_eq!(checkpoint.pending_batch(), Some(&batch));
}
