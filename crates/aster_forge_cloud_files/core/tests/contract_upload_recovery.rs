mod support;

use aster_forge_cloud_files_core::{
    BackendResult, CloudContentMetadata, CloudContentUploadBackend, CloudFilesStoreErrorKind,
    CloudItem, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
    ContentCacheKey, ContentEvictionBegin, ContentEvictionBlocker, ContentEvictionIntent,
    ContentEvictionOperationId, ContentEvictionTarget, ContentLeaseId, ContentRevision,
    ContentStorageEntry, ContentStorageMode, ContentStorageStore, ContentUploadChunk,
    ContentUploadChunkAck, ContentUploadIntent, ContentUploadSession, ContentUploadSessionId,
    ContentUploadState, ContentUploadStore, IdempotencyKey, LocalContentGeneration,
    LocalContentReference, LocalContentSnapshot, MetadataRevision, MutationRemoteOutcome,
    OperationId, SessionGeneration, StoreWriteStatus,
};
use async_trait::async_trait;
use bytes::Bytes;

use support::{
    SyntheticBackend, content_storage::MemoryContentStorageStore, failure_injection::FailurePoint,
};

fn revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes()).expect("revision fixture should be valid")
}

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn local_generation(value: u64) -> LocalContentGeneration {
    LocalContentGeneration::new(value).expect("local generation fixture should be valid")
}

fn operation_id(value: &str) -> OperationId {
    OperationId::new(value).expect("operation fixture should be valid")
}

fn idempotency_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("idempotency fixture should be valid")
}

fn upload_session(value: &str, accepted_offset: u64) -> ContentUploadSession {
    ContentUploadSession::new(
        ContentUploadSessionId::new(value).expect("upload session fixture should be valid"),
        accepted_offset,
    )
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

fn snapshot(
    backend: &SyntheticBackend,
    generation: u64,
    reference: &str,
    size: u64,
) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        backend.moved_file().key().clone(),
        local_generation(generation),
        LocalContentReference::new(reference).expect("local reference fixture should be valid"),
        size,
        None,
    )
}

fn snapshot_for_key(
    key: CloudItemKey,
    generation: u64,
    reference: &str,
    size: u64,
) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        key,
        local_generation(generation),
        LocalContentReference::new(reference).expect("local reference fixture should be valid"),
        size,
        None,
    )
}

fn upload_intent(
    operation: &str,
    key: ContentCacheKey,
    snapshot: LocalContentSnapshot,
) -> ContentUploadIntent {
    ContentUploadIntent::new(
        operation_id(operation),
        idempotency_key(&format!("idempotency-{operation}")),
        key,
        snapshot,
        ContentLeaseId::new(format!("lease-{operation}"))
            .expect("upload lease fixture should be valid"),
        generation(1),
    )
    .expect("upload intent should be valid")
}

fn upload_intent_with_ids(
    operation: &str,
    idempotency: &str,
    lease: &str,
    key: ContentCacheKey,
    snapshot: LocalContentSnapshot,
) -> ContentUploadIntent {
    ContentUploadIntent::new(
        operation_id(operation),
        idempotency_key(idempotency),
        key,
        snapshot,
        ContentLeaseId::new(lease).expect("upload lease fixture should be valid"),
        generation(1),
    )
    .expect("upload intent should be valid")
}

fn uploaded_item(backend: &SyntheticBackend, revision_value: &str, size: u64) -> CloudItem {
    let current = backend.moved_file();
    CloudItem::file(
        current.key().clone(),
        current
            .parent_id()
            .expect("synthetic file should have a parent")
            .clone(),
        current.name(),
        MetadataRevision::from_slice(format!("metadata-{revision_value}").as_bytes())
            .expect("metadata revision fixture should be valid"),
        CloudContentMetadata::new(revision(revision_value), None, size),
    )
    .expect("uploaded item fixture should be valid")
}

async fn create_dirty_entry(
    store: &MemoryContentStorageStore,
    key: ContentCacheKey,
    snapshot: LocalContentSnapshot,
) {
    store.activate_content_upload_generation(key.item_key().scope().clone(), generation(1));
    store
        .create_content_entry(ContentStorageEntry::new(
            key.clone(),
            ContentStorageMode::ProviderManaged,
            snapshot.size(),
        ))
        .await
        .expect("content entry creation should succeed");
    store
        .mark_content_dirty(&key, snapshot)
        .await
        .expect("dirty snapshot should be recorded");
}

#[derive(Debug, Default)]
struct SyntheticUploadBackend;

#[async_trait]
impl CloudContentUploadBackend for SyntheticUploadBackend {
    async fn start_upload(
        &self,
        _intent: &ContentUploadIntent,
    ) -> BackendResult<ContentUploadSession> {
        Ok(upload_session("synthetic-upload", 0))
    }

    async fn upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        let session = upload_session("synthetic-upload", chunk.offset());
        chunk
            .validate_for(intent, &session)
            .expect("synthetic upload chunk should satisfy the request contract");
        Ok(ContentUploadChunkAck::new(
            chunk.session_id().clone(),
            chunk.end_exclusive(),
        ))
    }

    async fn reconcile_upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        chunk
            .validate_for(intent, session)
            .expect("synthetic upload chunk should satisfy the recovery contract");
        Ok(ContentUploadChunkAck::new(
            chunk.session_id().clone(),
            chunk.end_exclusive(),
        ))
    }

    async fn commit_upload(
        &self,
        intent: &ContentUploadIntent,
        _session: &ContentUploadSession,
    ) -> BackendResult<MutationRemoteOutcome> {
        let fixture = SyntheticBackend::full();
        Ok(MutationRemoteOutcome::Committed {
            item: Some(uploaded_item(
                &fixture,
                "synthetic-committed",
                intent.snapshot().size(),
            )),
        })
    }

    async fn reconcile_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let fixture = SyntheticBackend::full();
        Ok(MutationRemoteOutcome::AlreadyCommitted {
            item: Some(uploaded_item(
                &fixture,
                "synthetic-reconciled",
                intent.snapshot().size(),
            )),
        })
    }
}

#[test]
fn local_content_generation_zero_is_reserved() {
    assert!(LocalContentGeneration::new(0).is_err());
}

#[tokio::test]
async fn upload_backend_requests_bind_chunks_to_immutable_generation_and_exact_ack() {
    let fixture = SyntheticBackend::full();
    let key = cache_key(&fixture, "backend-upload-v1");
    let source = snapshot(&fixture, 3, "snapshot-3", 8);
    let intent = upload_intent("backend-upload", key, source.clone());
    let backend: &dyn CloudContentUploadBackend = &SyntheticUploadBackend;
    let session = backend
        .start_upload(&intent)
        .await
        .expect("upload session should start");
    session
        .validate_for(&intent)
        .expect("backend session should remain within the immutable snapshot");
    let chunk = ContentUploadChunk::new(
        session.id().clone(),
        source.generation(),
        0,
        Bytes::from_static(b"12345678"),
        source.size(),
    )
    .expect("upload chunk should be valid");
    chunk
        .validate_for(&intent, &session)
        .expect("chunk should continue the exact immutable upload session");
    let ack = backend
        .upload_chunk(&intent, &chunk)
        .await
        .expect("chunk upload should succeed");
    ack.validate_for(&chunk)
        .expect("backend acknowledgement should match the exact chunk");
    assert_eq!(ack.accepted_offset(), 8);

    let wrong_ack = ContentUploadChunkAck::new(session.id().clone(), 7);
    assert!(wrong_ack.validate_for(&chunk).is_err());
    assert!(
        ContentUploadChunk::new(
            session.id().clone(),
            source.generation(),
            8,
            Bytes::from_static(b"x"),
            source.size(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn upload_intent_requires_current_dirty_snapshot_and_holds_eviction_lease() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "upload-lease-v1");
    let source = snapshot(&backend, 1, "snapshot-1", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let intent = upload_intent("upload-lease", key.clone(), source);
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("upload intent should persist");

    assert_eq!(
        store
            .begin_content_eviction(ContentEvictionIntent::new(
                ContentEvictionOperationId::new("upload-blocked-eviction")
                    .expect("eviction operation should be valid"),
                key,
                ContentEvictionTarget::ProviderCache,
                generation(1),
            ))
            .await
            .expect("blocked eviction should return a decision"),
        ContentEvictionBegin::Blocked {
            blockers: vec![
                ContentEvictionBlocker::Dirty,
                ContentEvictionBlocker::UploadLease,
            ]
        }
    );
}

#[tokio::test]
async fn resumable_checkpoint_is_monotonic_and_commit_requires_complete_snapshot() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "checkpoint-v1");
    let source = snapshot(&backend, 1, "checkpoint-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let intent = upload_intent("checkpoint", key, source);
    let operation = intent.operation_id().clone();
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("upload intent should persist");
    store
        .record_content_upload_session(&operation, upload_session("session-1", 4), generation(1))
        .await
        .expect("first checkpoint should persist");
    let error = store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect_err("partial upload must not start remote commit");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
    assert_eq!(
        store
            .record_content_upload_session(
                &operation,
                upload_session("session-1", 2),
                generation(1)
            )
            .await
            .expect("stale checkpoint should be fenced"),
        StoreWriteStatus::Fenced
    );
    assert_eq!(
        store
            .record_content_upload_session(
                &operation,
                upload_session("session-1", 8),
                generation(1)
            )
            .await
            .expect("complete checkpoint should advance"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .begin_content_upload_remote_commit(&operation, generation(1))
            .await
            .expect("complete upload may start remote commit"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn committed_upload_outcome_requires_current_item_metadata() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "missing-item-v1");
    let source = snapshot(&backend, 1, "missing-item-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let intent = upload_intent("missing-item", key, source);
    let operation = intent.operation_id().clone();
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("upload intent should persist");
    store
        .record_content_upload_session(
            &operation,
            upload_session("missing-item-session", 8),
            generation(1),
        )
        .await
        .expect("complete checkpoint should persist");
    store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect("remote commit should begin");
    let error = store
        .record_content_upload_remote_outcome(
            &operation,
            MutationRemoteOutcome::Committed { item: None },
            generation(1),
        )
        .await
        .expect_err("upload commit must return the current revision-bearing item");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);
}

#[tokio::test]
async fn remote_commit_unknown_is_reconciled_without_reuploading_bytes() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "unknown-upload-v1");
    let source = snapshot(&backend, 1, "unknown-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let intent = upload_intent("unknown-upload", key.clone(), source);
    let operation = intent.operation_id().clone();
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("upload intent should persist");
    store
        .record_content_upload_session(
            &operation,
            upload_session("unknown-session", 8),
            generation(1),
        )
        .await
        .expect("complete checkpoint should persist");
    store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect("remote commit marker should persist");
    store
        .record_content_upload_remote_outcome(
            &operation,
            MutationRemoteOutcome::RemoteOutcomeUnknown,
            generation(1),
        )
        .await
        .expect("unknown outcome should persist");
    assert_eq!(
        store
            .load_content_upload(&operation)
            .await
            .expect("upload load should succeed")
            .expect("upload should exist")
            .state(),
        ContentUploadState::RemoteOutcomeUnknown
    );

    store
        .record_content_upload_remote_outcome(
            &operation,
            MutationRemoteOutcome::AlreadyCommitted {
                item: Some(uploaded_item(&backend, "unknown-reconciled", 8)),
            },
            generation(1),
        )
        .await
        .expect("status reconciliation should prove existing commit");
    store
        .reconcile_content_upload_metadata(&operation, generation(1))
        .await
        .expect("known outcome should reconcile metadata");
    assert!(
        !store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .is_dirty()
    );
}

#[tokio::test]
async fn stale_upload_completion_never_clears_a_newer_local_generation() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "stale-completion-v1");
    let first = snapshot(&backend, 1, "snapshot-1", 8);
    create_dirty_entry(&store, key.clone(), first.clone()).await;
    let intent = upload_intent("stale-completion", key.clone(), first);
    let operation = intent.operation_id().clone();
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("first upload intent should persist");
    store
        .record_content_upload_session(
            &operation,
            upload_session("stale-session", 8),
            generation(1),
        )
        .await
        .expect("complete checkpoint should persist");
    store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect("remote commit should begin");
    store
        .record_content_upload_remote_outcome(
            &operation,
            MutationRemoteOutcome::Committed {
                item: Some(uploaded_item(&backend, "stale-committed", 8)),
            },
            generation(1),
        )
        .await
        .expect("remote outcome should persist");

    let newer = snapshot(&backend, 2, "snapshot-2", 8);
    store
        .mark_content_dirty(&key, newer.clone())
        .await
        .expect("newer local write should supersede the uploading snapshot");
    assert_eq!(
        store
            .reconcile_content_upload_metadata(&operation, generation(1))
            .await
            .expect("stale completion should be recorded but fenced"),
        StoreWriteStatus::Fenced
    );
    let entry = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert_eq!(entry.dirty_snapshot(), Some(&newer));
}

#[tokio::test]
async fn precondition_failure_keeps_local_content_dirty_for_conflict_policy() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "precondition-v1");
    let source = snapshot(&backend, 1, "precondition-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let intent = upload_intent("precondition", key.clone(), source.clone());
    let operation = intent.operation_id().clone();
    store
        .persist_content_upload_intent(intent, generation(1))
        .await
        .expect("upload intent should persist");
    store
        .record_content_upload_session(
            &operation,
            upload_session("precondition-session", 8),
            generation(1),
        )
        .await
        .expect("complete checkpoint should persist");
    store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect("remote commit should begin");
    store
        .record_content_upload_remote_outcome(
            &operation,
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: None,
                content_revision: Some(revision("remote-newer")),
            },
            generation(1),
        )
        .await
        .expect("precondition outcome should persist");
    store
        .reconcile_content_upload_metadata(&operation, generation(1))
        .await
        .expect("conflict metadata should reconcile without dropping local bytes");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .dirty_snapshot(),
        Some(&source)
    );
}

#[tokio::test]
async fn every_upload_persisted_transition_is_idempotent_after_lost_return() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "lost-upload-v1");
    let source = snapshot(&backend, 1, "lost-upload-source", 8);
    create_dirty_entry(&store, key, source.clone()).await;
    let intent = upload_intent("lost-upload", cache_key(&backend, "lost-upload-v1"), source);
    let operation = intent.operation_id().clone();

    store
        .failures()
        .arm(FailurePoint::AfterContentUploadIntentPersistBeforeReturn);
    store
        .persist_content_upload_intent(intent.clone(), generation(1))
        .await
        .expect_err("intent return should be lost");
    assert_eq!(
        store
            .persist_content_upload_intent(intent, generation(1))
            .await
            .expect("intent retry should find durable state"),
        StoreWriteStatus::AlreadyApplied
    );

    let session = upload_session("lost-upload-session", 8);
    store
        .failures()
        .arm(FailurePoint::AfterContentUploadCheckpointPersistBeforeReturn);
    store
        .record_content_upload_session(&operation, session.clone(), generation(1))
        .await
        .expect_err("checkpoint return should be lost");
    assert_eq!(
        store
            .record_content_upload_session(&operation, session, generation(1))
            .await
            .expect("checkpoint retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentUploadRemoteCommitPersistBeforeReturn);
    store
        .begin_content_upload_remote_commit(&operation, generation(1))
        .await
        .expect_err("remote commit marker return should be lost");
    assert_eq!(
        store
            .begin_content_upload_remote_commit(&operation, generation(1))
            .await
            .expect("remote commit marker retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    let outcome = MutationRemoteOutcome::Committed {
        item: Some(uploaded_item(&backend, "lost-committed", 8)),
    };
    store
        .failures()
        .arm(FailurePoint::AfterContentUploadOutcomePersistBeforeReturn);
    store
        .record_content_upload_remote_outcome(&operation, outcome.clone(), generation(1))
        .await
        .expect_err("outcome return should be lost");
    assert_eq!(
        store
            .record_content_upload_remote_outcome(&operation, outcome, generation(1))
            .await
            .expect("outcome retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentUploadMetadataPersistBeforeReturn);
    store
        .reconcile_content_upload_metadata(&operation, generation(1))
        .await
        .expect_err("metadata return should be lost");
    assert_eq!(
        store
            .reconcile_content_upload_metadata(&operation, generation(1))
            .await
            .expect("metadata retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterContentUploadCompletionPersistBeforeReturn);
    store
        .complete_content_upload(&operation, generation(1))
        .await
        .expect_err("completion return should be lost");
    assert_eq!(
        store
            .complete_content_upload(&operation, generation(1))
            .await
            .expect("completion retry should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert!(
        store
            .collect_upload_backlog(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .is_empty()
    );
}

#[tokio::test]
async fn upload_store_rejects_missing_stale_and_conflicting_dirty_snapshots() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let clean_key = cache_key(&backend, "clean-v1");
    store
        .create_content_entry(ContentStorageEntry::new(
            clean_key.clone(),
            ContentStorageMode::ProviderManaged,
            8,
        ))
        .await
        .expect("clean entry should be created");
    store.activate_content_upload_generation(clean_key.item_key().scope().clone(), generation(1));
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent("clean", clean_key, snapshot(&backend, 1, "clean-source", 8),),
                generation(1)
            )
            .await
            .expect_err("upload without dirty state should fail")
            .kind(),
        CloudFilesStoreErrorKind::InvalidTransition
    );

    let key = cache_key(&backend, "dirty-conflict-v1");
    let current = snapshot(&backend, 2, "current-source", 8);
    create_dirty_entry(&store, key.clone(), current).await;
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent(
                    "stale",
                    key.clone(),
                    snapshot(&backend, 1, "stale-source", 8),
                ),
                generation(1)
            )
            .await
            .expect("stale generation should be fenced"),
        StoreWriteStatus::Fenced
    );
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent(
                    "same-generation-different-bytes",
                    key,
                    snapshot(&backend, 2, "different-source", 8),
                ),
                generation(1)
            )
            .await
            .expect_err("different snapshot for current generation should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
}

#[tokio::test]
async fn upload_operation_idempotency_and_lease_id_conflicts_are_distinct() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "identity-conflicts-v1");
    let source = snapshot(&backend, 1, "identity-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let original = upload_intent_with_ids(
        "operation-a",
        "idempotency-a",
        "lease-a",
        key.clone(),
        source.clone(),
    );
    assert_eq!(
        store
            .persist_content_upload_intent(original.clone(), generation(1))
            .await
            .expect("first intent should persist"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .persist_content_upload_intent(original, generation(1))
            .await
            .expect("same intent should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent_with_ids(
                    "operation-a",
                    "idempotency-other",
                    "lease-other",
                    key.clone(),
                    source.clone(),
                ),
                generation(1)
            )
            .await
            .expect_err("same operation id with another intent should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent_with_ids(
                    "operation-b",
                    "idempotency-a",
                    "lease-b",
                    key.clone(),
                    source.clone(),
                ),
                generation(1)
            )
            .await
            .expect_err("reused idempotency key should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
    assert_eq!(
        store
            .persist_content_upload_intent(
                upload_intent_with_ids("operation-c", "idempotency-c", "lease-a", key, source,),
                generation(1)
            )
            .await
            .expect_err("reused lease id should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
}

#[tokio::test]
async fn concurrent_uploads_hold_independent_leases_until_each_completion() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "concurrent-v1");
    let source = snapshot(&backend, 1, "concurrent-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    let first = upload_intent_with_ids(
        "upload-a",
        "idempotency-a",
        "lease-a",
        key.clone(),
        source.clone(),
    );
    let second = upload_intent_with_ids(
        "upload-b",
        "idempotency-b",
        "lease-b",
        key.clone(),
        source.clone(),
    );
    for intent in [first.clone(), second.clone()] {
        store
            .persist_content_upload_intent(intent, generation(1))
            .await
            .expect("upload intent should persist");
    }
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .lease_counts()
            .upload,
        2
    );

    for intent in [&first, &second] {
        store
            .record_content_upload_session(
                intent.operation_id(),
                upload_session(&format!("session-{}", intent.operation_id().as_str()), 8),
                generation(1),
            )
            .await
            .expect("session should persist");
        store
            .begin_content_upload_remote_commit(intent.operation_id(), generation(1))
            .await
            .expect("remote commit should begin");
        store
            .record_content_upload_remote_outcome(
                intent.operation_id(),
                MutationRemoteOutcome::PreconditionFailed {
                    metadata_revision: None,
                    content_revision: None,
                },
                generation(1),
            )
            .await
            .expect("precondition outcome should persist");
        store
            .reconcile_content_upload_metadata(intent.operation_id(), generation(1))
            .await
            .expect("metadata should reconcile");
    }

    store
        .complete_content_upload(first.operation_id(), generation(1))
        .await
        .expect("first upload should complete");
    let after_first = store
        .load_content_entry(&key)
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert_eq!(after_first.lease_counts().upload, 1);
    assert!(after_first.is_dirty());
    assert_eq!(
        store
            .begin_content_eviction(ContentEvictionIntent::new(
                ContentEvictionOperationId::new("still-uploading")
                    .expect("operation fixture should be valid"),
                key.clone(),
                ContentEvictionTarget::ProviderCache,
                generation(1),
            ))
            .await
            .expect("eviction decision should succeed"),
        ContentEvictionBegin::Blocked {
            blockers: vec![
                ContentEvictionBlocker::Dirty,
                ContentEvictionBlocker::UploadLease,
            ]
        }
    );
    store
        .complete_content_upload(second.operation_id(), generation(1))
        .await
        .expect("second upload should complete");
    assert_eq!(
        store
            .load_content_entry(&key)
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .lease_counts()
            .upload,
        0
    );
}

#[tokio::test]
async fn upload_recovery_is_scope_filtered_sorted_and_excludes_completed_records() {
    let backend = SyntheticBackend::full();
    let store = MemoryContentStorageStore::default();
    let key = cache_key(&backend, "recovery-v1");
    let source = snapshot(&backend, 1, "recovery-source", 8);
    create_dirty_entry(&store, key.clone(), source.clone()).await;
    for operation in ["z-upload", "a-upload"] {
        store
            .persist_content_upload_intent(
                upload_intent_with_ids(
                    operation,
                    &format!("idem-{operation}"),
                    &format!("lease-{operation}"),
                    key.clone(),
                    source.clone(),
                ),
                generation(1),
            )
            .await
            .expect("upload intent should persist");
    }
    let other_key = scoped_cache_key("other-namespace", "other-root", "other-file", "other-v1");
    let other_source =
        snapshot_for_key(other_key.item_key().clone(), 1, "other-recovery-source", 8);
    create_dirty_entry(&store, other_key.clone(), other_source.clone()).await;
    store
        .persist_content_upload_intent(
            upload_intent_with_ids(
                "m-other-upload",
                "idem-other-upload",
                "lease-other-upload",
                other_key.clone(),
                other_source,
            ),
            generation(1),
        )
        .await
        .expect("other-scope upload intent should persist");
    assert_eq!(
        store
            .collect_upload_backlog(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["a-upload", "z-upload"]
    );
    assert_eq!(
        store
            .collect_upload_backlog(other_key.item_key().scope())
            .await
            .expect("other-scope recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["m-other-upload"]
    );

    let completed = operation_id("a-upload");
    store
        .record_content_upload_session(
            &completed,
            upload_session("completed-session", 8),
            generation(1),
        )
        .await
        .expect("session should persist");
    store
        .begin_content_upload_remote_commit(&completed, generation(1))
        .await
        .expect("remote commit should begin");
    store
        .record_content_upload_remote_outcome(
            &completed,
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: None,
                content_revision: None,
            },
            generation(1),
        )
        .await
        .expect("outcome should persist");
    store
        .reconcile_content_upload_metadata(&completed, generation(1))
        .await
        .expect("metadata should reconcile");
    store
        .complete_content_upload(&completed, generation(1))
        .await
        .expect("upload should complete");
    assert_eq!(
        store
            .collect_upload_backlog(backend.scope())
            .await
            .expect("recovery scan should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["z-upload"]
    );
}
