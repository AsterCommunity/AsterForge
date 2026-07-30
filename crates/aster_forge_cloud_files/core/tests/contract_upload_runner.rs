mod support;

use std::{collections::VecDeque, sync::Mutex};

use aster_forge_cloud_files_core::{
    BackendResult, CloudBackendError, CloudBackendErrorKind, CloudContentMetadata,
    CloudContentUploadBackend, CloudFilesCoreError, CloudFilesStoreError, CloudFilesStoreErrorKind,
    CloudItem, ContentCacheKey, ContentLeaseId, ContentRevision, ContentStorageEntry,
    ContentStorageMode, ContentStorageStore, ContentUploadChunk, ContentUploadChunkAck,
    ContentUploadIntent, ContentUploadRunError, ContentUploadRunOutcome, ContentUploadRunner,
    ContentUploadSession, ContentUploadSessionId, ContentUploadState, ContentUploadStore,
    IdempotencyKey, LocalContentGeneration, LocalContentReference, LocalContentSnapshot,
    LocalContentSnapshotReader, MetadataRevision, MutationRemoteOutcome, OperationId,
    SessionGeneration, StoreResult,
};
use async_trait::async_trait;
use bytes::Bytes;

use support::{
    SyntheticBackend, content_storage::MemoryContentStorageStore, failure_injection::FailurePoint,
};

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn local_generation(value: u64) -> LocalContentGeneration {
    LocalContentGeneration::new(value).expect("local generation fixture should be valid")
}

fn cache_key(backend: &SyntheticBackend) -> ContentCacheKey {
    ContentCacheKey::new(
        backend.moved_file().key().clone(),
        ContentRevision::from_slice(b"upload-runner-base")
            .expect("content revision fixture should be valid"),
    )
}

fn snapshot(backend: &SyntheticBackend, bytes: &Bytes) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        backend.moved_file().key().clone(),
        local_generation(1),
        LocalContentReference::new("upload-runner-source")
            .expect("local reference fixture should be valid"),
        u64::try_from(bytes.len()).expect("fixture length should fit u64"),
        None,
    )
}

fn intent(backend: &SyntheticBackend, bytes: &Bytes) -> ContentUploadIntent {
    ContentUploadIntent::new(
        OperationId::new("upload-runner-operation").expect("operation fixture should be valid"),
        IdempotencyKey::new("upload-runner-idempotency")
            .expect("idempotency fixture should be valid"),
        cache_key(backend),
        snapshot(backend, bytes),
        ContentLeaseId::new("upload-runner-lease").expect("lease fixture should be valid"),
        generation(1),
    )
    .expect("upload intent fixture should be valid")
}

async fn prepare_store(store: &MemoryContentStorageStore, intent: &ContentUploadIntent) {
    store.activate_content_upload_generation(
        intent.base_cache_key().item_key().scope().clone(),
        generation(1),
    );
    store
        .create_content_entry(ContentStorageEntry::new(
            intent.base_cache_key().clone(),
            ContentStorageMode::ProviderManaged,
            intent.snapshot().size(),
        ))
        .await
        .expect("content entry should be created");
    store
        .mark_content_dirty(intent.base_cache_key(), intent.snapshot().clone())
        .await
        .expect("dirty snapshot should be recorded");
}

fn upload_session(accepted_offset: u64) -> ContentUploadSession {
    ContentUploadSession::new(
        ContentUploadSessionId::new("scripted-session")
            .expect("upload session fixture should be valid"),
        accepted_offset,
    )
}

async fn persist_upload(store: &MemoryContentStorageStore, intent: &ContentUploadIntent) {
    prepare_store(store, intent).await;
    store
        .persist_content_upload_intent(intent.clone(), generation(1))
        .await
        .expect("upload intent should persist");
}

fn uploaded_item(intent: &ContentUploadIntent, revision: &str) -> CloudItem {
    let fixture = SyntheticBackend::full();
    let current = fixture.moved_file();
    CloudItem::file(
        intent.snapshot().item_key().clone(),
        current
            .parent_id()
            .expect("synthetic file should have a parent")
            .clone(),
        current.name(),
        MetadataRevision::from_slice(format!("metadata-{revision}").as_bytes())
            .expect("metadata revision fixture should be valid"),
        CloudContentMetadata::new(
            ContentRevision::from_slice(revision.as_bytes())
                .expect("content revision fixture should be valid"),
            None,
            intent.snapshot().size(),
        ),
    )
    .expect("uploaded item fixture should be valid")
}

#[derive(Debug, Default)]
struct ReaderControls {
    partial_once: bool,
    fail_once: bool,
}

#[derive(Debug)]
struct MemorySnapshotReader {
    bytes: Bytes,
    reads: Mutex<Vec<(u64, u64)>>,
    controls: Mutex<ReaderControls>,
}

impl MemorySnapshotReader {
    fn new(bytes: Bytes) -> Self {
        Self {
            bytes,
            reads: Mutex::new(Vec::new()),
            controls: Mutex::new(ReaderControls::default()),
        }
    }

    fn partial_once(&self) {
        self.controls
            .lock()
            .expect("reader controls mutex should not be poisoned")
            .partial_once = true;
    }

    fn fail_once(&self) {
        self.controls
            .lock()
            .expect("reader controls mutex should not be poisoned")
            .fail_once = true;
    }

    fn reads(&self) -> Vec<(u64, u64)> {
        self.reads
            .lock()
            .expect("reader calls mutex should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl LocalContentSnapshotReader for MemorySnapshotReader {
    async fn read_snapshot(
        &self,
        _snapshot: &LocalContentSnapshot,
        offset: u64,
        length: u64,
    ) -> StoreResult<Bytes> {
        self.reads
            .lock()
            .expect("reader calls mutex should not be poisoned")
            .push((offset, length));
        let mut controls = self
            .controls
            .lock()
            .expect("reader controls mutex should not be poisoned");
        if std::mem::take(&mut controls.fail_once) {
            return Err(CloudFilesStoreError::new(
                CloudFilesStoreErrorKind::PersistenceFailure,
                "injected immutable source failure",
            ));
        }
        let length = if std::mem::take(&mut controls.partial_once) {
            length.saturating_sub(1)
        } else {
            length
        };
        drop(controls);
        let start = usize::try_from(offset).map_err(|_| {
            CloudFilesStoreError::new(
                CloudFilesStoreErrorKind::PersistenceFailure,
                "source offset exceeded usize",
            )
        })?;
        let end = usize::try_from(offset + length).map_err(|_| {
            CloudFilesStoreError::new(
                CloudFilesStoreErrorKind::PersistenceFailure,
                "source end exceeded usize",
            )
        })?;
        Ok(self.bytes.slice(start..end))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitPlan {
    Committed,
    Unknown,
    PreconditionFailed,
    Fail,
}

#[derive(Debug)]
struct UploadBackendState {
    remote: Vec<u8>,
    start_calls: usize,
    chunks: Vec<(u64, Bytes)>,
    commit_calls: usize,
    reconcile_calls: usize,
    start_offset_override: Option<u64>,
    bad_ack_once: bool,
    chunk_fail_once: bool,
    chunk_conflict_once: bool,
    chunk_reconcile_calls: usize,
    commit_plans: VecDeque<CommitPlan>,
    reconcile_plans: VecDeque<CommitPlan>,
}

impl Default for UploadBackendState {
    fn default() -> Self {
        Self {
            remote: Vec::new(),
            start_calls: 0,
            chunks: Vec::new(),
            commit_calls: 0,
            reconcile_calls: 0,
            start_offset_override: None,
            bad_ack_once: false,
            chunk_fail_once: false,
            chunk_conflict_once: false,
            chunk_reconcile_calls: 0,
            commit_plans: VecDeque::from([CommitPlan::Committed]),
            reconcile_plans: VecDeque::from([CommitPlan::Committed]),
        }
    }
}

#[derive(Debug, Default)]
struct ScriptedUploadBackend {
    state: Mutex<UploadBackendState>,
}

impl ScriptedUploadBackend {
    fn with_remote(bytes: &[u8]) -> Self {
        let state = UploadBackendState {
            remote: bytes.to_vec(),
            ..UploadBackendState::default()
        };
        Self {
            state: Mutex::new(state),
        }
    }

    fn set_start_offset(&self, offset: u64) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .start_offset_override = Some(offset);
    }

    fn bad_ack_once(&self) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .bad_ack_once = true;
    }

    fn fail_chunk_once(&self) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .chunk_fail_once = true;
    }

    fn conflict_chunk_once(&self) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .chunk_conflict_once = true;
    }

    fn set_commit_plans(&self, plans: impl IntoIterator<Item = CommitPlan>) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .commit_plans = plans.into_iter().collect();
    }

    fn set_reconcile_plans(&self, plans: impl IntoIterator<Item = CommitPlan>) {
        self.state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .reconcile_plans = plans.into_iter().collect();
    }
}

fn plan_outcome(
    plan: CommitPlan,
    intent: &ContentUploadIntent,
) -> BackendResult<MutationRemoteOutcome> {
    match plan {
        CommitPlan::Committed => Ok(MutationRemoteOutcome::Committed {
            item: Some(uploaded_item(intent, "uploaded-content")),
        }),
        CommitPlan::Unknown => Ok(MutationRemoteOutcome::RemoteOutcomeUnknown),
        CommitPlan::PreconditionFailed => Ok(MutationRemoteOutcome::PreconditionFailed {
            metadata_revision: None,
            content_revision: Some(
                ContentRevision::from_slice(b"remote-newer")
                    .expect("revision fixture should be valid"),
            ),
        }),
        CommitPlan::Fail => Err(CloudBackendError::retryable(
            CloudBackendErrorKind::TemporarilyUnavailable,
        )),
    }
}

#[async_trait]
impl CloudContentUploadBackend for ScriptedUploadBackend {
    async fn start_upload(
        &self,
        _intent: &ContentUploadIntent,
    ) -> BackendResult<ContentUploadSession> {
        let mut state = self
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        state.start_calls += 1;
        let accepted_offset = state.start_offset_override.unwrap_or(
            u64::try_from(state.remote.len())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?,
        );
        Ok(ContentUploadSession::new(
            ContentUploadSessionId::new("scripted-session")
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::Internal))?,
            accepted_offset,
        ))
    }

    async fn upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        let mut state = self
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        if std::mem::take(&mut state.chunk_fail_once) {
            return Err(CloudBackendError::retryable(
                CloudBackendErrorKind::TemporarilyUnavailable,
            ));
        }
        let durable_session = ContentUploadSession::new(chunk.session_id().clone(), chunk.offset());
        chunk
            .validate_for(intent, &durable_session)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let start = usize::try_from(chunk.offset())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let end = usize::try_from(chunk.end_exclusive())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        if start == state.remote.len() {
            state.remote.extend_from_slice(chunk.bytes());
        } else if end <= state.remote.len() && state.remote[start..end] == chunk.bytes()[..] {
            // Concurrent/restarted replay of the exact immutable range is idempotent.
        } else {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        state.chunks.push((chunk.offset(), chunk.bytes().clone()));
        if std::mem::take(&mut state.chunk_conflict_once) {
            return Err(CloudBackendError::new(CloudBackendErrorKind::Conflict));
        }
        let accepted_offset = if std::mem::take(&mut state.bad_ack_once) {
            chunk.offset()
        } else {
            chunk.end_exclusive()
        };
        Ok(ContentUploadChunkAck::new(
            chunk.session_id().clone(),
            accepted_offset,
        ))
    }

    async fn reconcile_upload_chunk(
        &self,
        _intent: &ContentUploadIntent,
        session: &ContentUploadSession,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        let mut state = self
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        state.chunk_reconcile_calls += 1;
        let start = usize::try_from(chunk.offset())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let end = usize::try_from(chunk.end_exclusive())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        if end > state.remote.len() || state.remote.get(start..end) != Some(chunk.bytes()) {
            return Err(CloudBackendError::new(CloudBackendErrorKind::Conflict));
        }
        Ok(ContentUploadChunkAck::new(
            session.id().clone(),
            chunk.end_exclusive(),
        ))
    }

    async fn commit_upload(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
    ) -> BackendResult<MutationRemoteOutcome> {
        session
            .validate_for(intent)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let mut state = self
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        state.commit_calls += 1;
        let plan = state
            .commit_plans
            .pop_front()
            .unwrap_or(CommitPlan::Committed);
        drop(state);
        plan_outcome(plan, intent)
    }

    async fn reconcile_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let mut state = self
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        state.reconcile_calls += 1;
        let plan = state
            .reconcile_plans
            .pop_front()
            .unwrap_or(CommitPlan::Committed);
        drop(state);
        plan_outcome(plan, intent)
    }
}

#[test]
fn runner_rejects_zero_chunk_size_and_preserves_configured_value() {
    assert!(matches!(
        ContentUploadRunner::new(0),
        Err(CloudFilesCoreError::InvalidContentUploadTransition { .. })
    ));
    assert_eq!(
        ContentUploadRunner::new(4096)
            .expect("runner config should be valid")
            .chunk_size(),
        4096
    );
}

#[tokio::test]
async fn runner_uploads_exact_chunks_and_completes_metadata_and_lease() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"abcdefghij");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes.clone());
    let backend = ScriptedUploadBackend::default();
    let runner = ContentUploadRunner::new(4).expect("runner config should be valid");

    let outcome = runner
        .submit(intent.clone(), generation(1), &store, &backend, &source)
        .await
        .expect("upload should complete");
    assert_eq!(outcome, ContentUploadRunOutcome::Completed);
    assert_eq!(source.reads(), vec![(0, 4), (4, 4), (8, 2)]);
    {
        let backend_state = backend
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned");
        assert_eq!(backend_state.remote, bytes);
        assert_eq!(backend_state.start_calls, 1);
        assert_eq!(backend_state.commit_calls, 1);
    }
    let record = store
        .load_content_upload(intent.operation_id())
        .await
        .expect("upload load should succeed")
        .expect("upload record should exist");
    assert_eq!(record.state(), ContentUploadState::Completed);
    let entry = store
        .load_content_entry(intent.base_cache_key())
        .await
        .expect("entry load should succeed")
        .expect("entry should exist");
    assert!(!entry.is_dirty());
    assert_eq!(entry.lease_counts().upload, 0);
}

#[tokio::test]
async fn runner_reconciles_a_chunk_accepted_before_checkpoint() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"replayed-chunk");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    backend.conflict_chunk_once();

    assert_eq!(
        ContentUploadRunner::new(64)
            .expect("runner config should be valid")
            .submit(intent.clone(), generation(1), &store, &backend, &source)
            .await
            .expect("accepted chunk should be reconciled"),
        ContentUploadRunOutcome::Completed
    );
    let state = backend
        .state
        .lock()
        .expect("upload backend mutex should not be poisoned");
    assert_eq!(state.chunk_reconcile_calls, 1);
}

#[tokio::test]
async fn empty_snapshot_commits_without_source_reads_or_chunks() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::new();
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();

    let outcome = ContentUploadRunner::new(3)
        .expect("runner config should be valid")
        .submit(intent, generation(1), &store, &backend, &source)
        .await
        .expect("empty upload should complete");
    assert_eq!(outcome, ContentUploadRunOutcome::Completed);
    assert!(source.reads().is_empty());
    let state = backend
        .state
        .lock()
        .expect("upload backend mutex should not be poisoned");
    assert!(state.chunks.is_empty());
    assert_eq!(state.commit_calls, 1);
}

#[tokio::test]
async fn runner_resumes_from_durable_offset_without_starting_or_rereading_prefix() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"abcdefghij");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    store
        .persist_content_upload_intent(intent.clone(), generation(1))
        .await
        .expect("intent should persist");
    store
        .record_content_upload_session(
            intent.operation_id(),
            ContentUploadSession::new(
                ContentUploadSessionId::new("scripted-session")
                    .expect("session fixture should be valid"),
                4,
            ),
            generation(1),
        )
        .await
        .expect("checkpoint should persist");
    let source = MemorySnapshotReader::new(bytes.clone());
    let backend = ScriptedUploadBackend::with_remote(b"abcd");

    ContentUploadRunner::new(4)
        .expect("runner config should be valid")
        .resume(
            intent.operation_id(),
            generation(1),
            &store,
            &backend,
            &source,
        )
        .await
        .expect("resumed upload should complete");
    assert_eq!(source.reads(), vec![(4, 4), (8, 2)]);
    let state = backend
        .state
        .lock()
        .expect("upload backend mutex should not be poisoned");
    assert_eq!(state.start_calls, 0);
    assert_eq!(state.remote, bytes);
}

#[tokio::test]
async fn unknown_commit_stops_then_reconciles_without_reupload_or_recommit() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"unknown");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    backend.set_commit_plans([CommitPlan::Unknown]);
    backend.set_reconcile_plans([CommitPlan::Unknown, CommitPlan::Committed]);
    let runner = ContentUploadRunner::new(64).expect("runner config should be valid");

    assert_eq!(
        runner
            .submit(intent.clone(), generation(1), &store, &backend, &source,)
            .await
            .expect("unknown commit should be durable"),
        ContentUploadRunOutcome::RemoteOutcomePending
    );
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("still-unknown reconcile should stop"),
        ContentUploadRunOutcome::RemoteOutcomePending
    );
    assert!(matches!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("known reconcile should complete"),
        ContentUploadRunOutcome::Completed
    ));
    let state = backend
        .state
        .lock()
        .expect("upload backend mutex should not be poisoned");
    assert_eq!(state.chunks.len(), 1);
    assert_eq!(state.commit_calls, 1);
    assert_eq!(state.reconcile_calls, 2);
}

#[tokio::test]
async fn unknown_recovery_rejects_a_store_status_without_durable_progress() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"unknown-store-progress");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    backend.set_commit_plans([CommitPlan::Unknown]);
    backend.set_reconcile_plans([CommitPlan::Unknown]);
    let runner = ContentUploadRunner::new(64).expect("runner config should be valid");

    assert_eq!(
        runner
            .submit(intent.clone(), generation(1), &store, &backend, &source,)
            .await
            .expect("unknown commit should be durable"),
        ContentUploadRunOutcome::RemoteOutcomePending
    );
    store.stall_next_content_upload_remote_outcome();

    assert!(matches!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await,
        Err(ContentUploadRunError::Contract(_))
    ));
}

#[tokio::test]
async fn precondition_failure_completes_but_preserves_dirty_snapshot() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"conflict");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    backend.set_commit_plans([CommitPlan::PreconditionFailed]);

    let outcome = ContentUploadRunner::new(64)
        .expect("runner config should be valid")
        .submit(intent.clone(), generation(1), &store, &backend, &source)
        .await
        .expect("precondition outcome should complete locally");
    assert_eq!(outcome, ContentUploadRunOutcome::Completed);
    assert_eq!(
        store
            .load_content_entry(intent.base_cache_key())
            .await
            .expect("entry load should succeed")
            .expect("entry should exist")
            .dirty_snapshot(),
        Some(intent.snapshot())
    );
}

#[tokio::test]
async fn source_backend_and_ack_failures_preserve_the_last_durable_checkpoint() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"abcdefgh");

    for failure in ["source", "chunk", "ack"] {
        let intent = ContentUploadIntent::new(
            OperationId::new(format!("runner-{failure}")).expect("operation should be valid"),
            IdempotencyKey::new(format!("runner-{failure}")).expect("idempotency should be valid"),
            cache_key(&fixture),
            snapshot(&fixture, &bytes),
            ContentLeaseId::new(format!("runner-{failure}")).expect("lease should be valid"),
            generation(1),
        )
        .expect("intent should be valid");
        let store = MemoryContentStorageStore::default();
        prepare_store(&store, &intent).await;
        let source = MemorySnapshotReader::new(bytes.clone());
        let backend = ScriptedUploadBackend::default();
        match failure {
            "source" => source.fail_once(),
            "chunk" => backend.fail_chunk_once(),
            "ack" => backend.bad_ack_once(),
            _ => unreachable!("failure fixture is exhaustive"),
        }
        let error = ContentUploadRunner::new(4)
            .expect("runner config should be valid")
            .submit(intent.clone(), generation(1), &store, &backend, &source)
            .await
            .expect_err("injected failure should stop the runner");
        match failure {
            "source" => assert!(matches!(error, ContentUploadRunError::Store(_))),
            "chunk" => assert!(matches!(error, ContentUploadRunError::Backend(_))),
            "ack" => assert!(matches!(error, ContentUploadRunError::Contract(_))),
            _ => unreachable!("failure fixture is exhaustive"),
        }
        let record = store
            .load_content_upload(intent.operation_id())
            .await
            .expect("upload load should succeed")
            .expect("upload record should exist");
        assert_eq!(record.state(), ContentUploadState::Uploading);
        assert_eq!(
            record
                .session()
                .expect("uploading record should have a session")
                .accepted_offset(),
            0
        );
    }
}

#[tokio::test]
async fn invalid_start_offset_and_partial_source_are_contract_failures() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"abcdef");
    for failure in ["offset", "partial"] {
        let intent = ContentUploadIntent::new(
            OperationId::new(format!("invalid-{failure}")).expect("operation should be valid"),
            IdempotencyKey::new(format!("invalid-{failure}")).expect("idempotency should be valid"),
            cache_key(&fixture),
            snapshot(&fixture, &bytes),
            ContentLeaseId::new(format!("invalid-{failure}")).expect("lease should be valid"),
            generation(1),
        )
        .expect("intent should be valid");
        let store = MemoryContentStorageStore::default();
        prepare_store(&store, &intent).await;
        let source = MemorySnapshotReader::new(bytes.clone());
        let backend = ScriptedUploadBackend::default();
        if failure == "offset" {
            backend.set_start_offset(7);
        } else {
            source.partial_once();
        }
        assert!(matches!(
            ContentUploadRunner::new(4)
                .expect("runner config should be valid")
                .submit(intent, generation(1), &store, &backend, &source)
                .await,
            Err(ContentUploadRunError::Contract(_))
        ));
    }
}

#[tokio::test]
async fn runner_rejects_a_store_status_that_makes_no_durable_progress() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"stalled-store");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    store.stall_next_content_upload_session();
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();

    assert!(matches!(
        ContentUploadRunner::new(4)
            .expect("runner config should be valid")
            .submit(intent.clone(), generation(1), &store, &backend, &source,)
            .await,
        Err(ContentUploadRunError::Contract(_))
    ));
    assert_eq!(
        store
            .load_content_upload(intent.operation_id())
            .await
            .expect("upload load should succeed")
            .expect("upload record should exist")
            .state(),
        ContentUploadState::IntentPersisted
    );
}

#[tokio::test]
async fn every_runner_store_transition_recovers_after_a_lost_persist_return() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"abcdefgh");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    let runner = ContentUploadRunner::new(4).expect("runner config should be valid");

    store
        .failures()
        .arm(FailurePoint::AfterContentUploadIntentPersistBeforeReturn);
    assert!(matches!(
        runner
            .submit(intent.clone(), generation(1), &store, &backend, &source,)
            .await,
        Err(ContentUploadRunError::Store(_))
    ));
    assert_eq!(
        store
            .load_content_upload(intent.operation_id())
            .await
            .expect("upload load should succeed")
            .expect("intent should be durable")
            .state(),
        ContentUploadState::IntentPersisted
    );

    for (failure, expected_state) in [
        (
            FailurePoint::AfterContentUploadCheckpointPersistBeforeReturn,
            ContentUploadState::Uploading,
        ),
        (
            FailurePoint::AfterContentUploadRemoteCommitPersistBeforeReturn,
            ContentUploadState::RemoteCommitting,
        ),
        (
            FailurePoint::AfterContentUploadOutcomePersistBeforeReturn,
            ContentUploadState::RemoteOutcomeKnown,
        ),
        (
            FailurePoint::AfterContentUploadMetadataPersistBeforeReturn,
            ContentUploadState::MetadataReconciled,
        ),
        (
            FailurePoint::AfterContentUploadCompletionPersistBeforeReturn,
            ContentUploadState::Completed,
        ),
    ] {
        store.failures().arm(failure);
        assert!(matches!(
            runner
                .resume(
                    intent.operation_id(),
                    generation(1),
                    &store,
                    &backend,
                    &source,
                )
                .await,
            Err(ContentUploadRunError::Store(_))
        ));
        assert_eq!(
            store
                .load_content_upload(intent.operation_id())
                .await
                .expect("upload load should succeed")
                .expect("upload record should exist")
                .state(),
            expected_state
        );
    }

    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("completed marker replay should succeed"),
        ContentUploadRunOutcome::Completed
    );
    let state = backend
        .state
        .lock()
        .expect("upload backend mutex should not be poisoned");
    assert_eq!(
        state
            .chunks
            .iter()
            .map(|(offset, _)| *offset)
            .collect::<Vec<_>>(),
        vec![0, 4]
    );
}

#[tokio::test]
async fn remote_commit_failure_retries_idempotently_from_remote_committing() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"commit-retry");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    backend.set_commit_plans([CommitPlan::Fail, CommitPlan::Committed]);
    let runner = ContentUploadRunner::new(64).expect("runner config should be valid");

    assert!(matches!(
        runner
            .submit(intent.clone(), generation(1), &store, &backend, &source,)
            .await,
        Err(ContentUploadRunError::Backend(_))
    ));
    assert_eq!(
        store
            .load_content_upload(intent.operation_id())
            .await
            .expect("upload load should succeed")
            .expect("upload record should exist")
            .state(),
        ContentUploadState::RemoteCommitting
    );
    assert!(matches!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("commit retry should complete"),
        ContentUploadRunOutcome::Completed
    ));
    assert_eq!(
        backend
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .commit_calls,
        2
    );
}

#[tokio::test]
async fn newer_session_fences_old_executor_and_resumes_the_same_immutable_intent() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"generation-fence");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    store
        .persist_content_upload_intent(intent.clone(), generation(1))
        .await
        .expect("intent should persist");
    store.activate_content_upload_generation(
        intent.base_cache_key().item_key().scope().clone(),
        generation(2),
    );
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    let runner = ContentUploadRunner::new(64).expect("runner config should be valid");

    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("old executor should be fenced"),
        ContentUploadRunOutcome::Fenced
    );
    assert_eq!(
        backend
            .state
            .lock()
            .expect("upload backend mutex should not be poisoned")
            .start_calls,
        1,
        "the remote start may race before the atomic store fence"
    );
    assert!(matches!(
        runner
            .resume(
                intent.operation_id(),
                generation(2),
                &store,
                &backend,
                &source,
            )
            .await
            .expect("new executor should resume old intent"),
        ContentUploadRunOutcome::Completed
    ));
}

#[tokio::test]
async fn submit_reports_fenced_when_the_upload_generation_is_no_longer_active() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"stale-submit");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    store.activate_content_upload_generation(
        intent.base_cache_key().item_key().scope().clone(),
        generation(2),
    );

    assert_eq!(
        ContentUploadRunner::new(4)
            .expect("runner config should be valid")
            .submit(
                intent,
                generation(1),
                &store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes),
            )
            .await
            .expect("stale submit should be classified"),
        ContentUploadRunOutcome::Fenced
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the fencing contract exercises every durable upload transition in one scenario matrix"
)]
async fn newer_generation_fences_every_post_session_upload_transition() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"fence-every-stage");
    let intent = intent(&fixture, &bytes);
    let scope = intent.base_cache_key().item_key().scope().clone();
    let runner = ContentUploadRunner::new(4).expect("runner config should be valid");

    let checkpoint_store = MemoryContentStorageStore::default();
    persist_upload(&checkpoint_store, &intent).await;
    checkpoint_store
        .record_content_upload_session(intent.operation_id(), upload_session(0), generation(1))
        .await
        .expect("uploading state should persist");
    checkpoint_store.activate_content_upload_generation(scope.clone(), generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &checkpoint_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes.clone()),
            )
            .await
            .expect("checkpoint transition should be fenced"),
        ContentUploadRunOutcome::Fenced
    );

    let commit_begin_store = MemoryContentStorageStore::default();
    persist_upload(&commit_begin_store, &intent).await;
    commit_begin_store
        .record_content_upload_session(
            intent.operation_id(),
            upload_session(intent.snapshot().size()),
            generation(1),
        )
        .await
        .expect("complete session should persist");
    commit_begin_store.activate_content_upload_generation(scope.clone(), generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &commit_begin_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes.clone()),
            )
            .await
            .expect("commit begin should be fenced"),
        ContentUploadRunOutcome::Fenced
    );

    let outcome_store = MemoryContentStorageStore::default();
    persist_upload(&outcome_store, &intent).await;
    outcome_store
        .record_content_upload_session(
            intent.operation_id(),
            upload_session(intent.snapshot().size()),
            generation(1),
        )
        .await
        .expect("complete session should persist");
    outcome_store
        .begin_content_upload_remote_commit(intent.operation_id(), generation(1))
        .await
        .expect("remote commit should begin");
    outcome_store.activate_content_upload_generation(scope.clone(), generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &outcome_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes.clone()),
            )
            .await
            .expect("outcome transition should be fenced"),
        ContentUploadRunOutcome::Fenced
    );

    let unknown_store = MemoryContentStorageStore::default();
    persist_upload(&unknown_store, &intent).await;
    unknown_store
        .record_content_upload_session(
            intent.operation_id(),
            upload_session(intent.snapshot().size()),
            generation(1),
        )
        .await
        .expect("complete session should persist");
    unknown_store
        .begin_content_upload_remote_commit(intent.operation_id(), generation(1))
        .await
        .expect("remote commit should begin");
    unknown_store
        .record_content_upload_remote_outcome(
            intent.operation_id(),
            MutationRemoteOutcome::RemoteOutcomeUnknown,
            generation(1),
        )
        .await
        .expect("unknown outcome should persist");
    unknown_store.activate_content_upload_generation(scope.clone(), generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &unknown_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes.clone()),
            )
            .await
            .expect("unknown reconciliation should be fenced"),
        ContentUploadRunOutcome::Fenced
    );

    let metadata_store = MemoryContentStorageStore::default();
    persist_upload(&metadata_store, &intent).await;
    metadata_store
        .record_content_upload_session(
            intent.operation_id(),
            upload_session(intent.snapshot().size()),
            generation(1),
        )
        .await
        .expect("complete session should persist");
    metadata_store
        .begin_content_upload_remote_commit(intent.operation_id(), generation(1))
        .await
        .expect("remote commit should begin");
    metadata_store
        .record_content_upload_remote_outcome(
            intent.operation_id(),
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: None,
                content_revision: None,
            },
            generation(1),
        )
        .await
        .expect("known outcome should persist");
    metadata_store.activate_content_upload_generation(scope.clone(), generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &metadata_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes.clone()),
            )
            .await
            .expect("metadata transition should be fenced"),
        ContentUploadRunOutcome::Fenced
    );

    let completion_store = MemoryContentStorageStore::default();
    persist_upload(&completion_store, &intent).await;
    completion_store
        .record_content_upload_session(
            intent.operation_id(),
            upload_session(intent.snapshot().size()),
            generation(1),
        )
        .await
        .expect("complete session should persist");
    completion_store
        .begin_content_upload_remote_commit(intent.operation_id(), generation(1))
        .await
        .expect("remote commit should begin");
    completion_store
        .record_content_upload_remote_outcome(
            intent.operation_id(),
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: None,
                content_revision: None,
            },
            generation(1),
        )
        .await
        .expect("known outcome should persist");
    completion_store
        .reconcile_content_upload_metadata(intent.operation_id(), generation(1))
        .await
        .expect("metadata should reconcile");
    completion_store.activate_content_upload_generation(scope, generation(2));
    assert_eq!(
        runner
            .resume(
                intent.operation_id(),
                generation(1),
                &completion_store,
                &ScriptedUploadBackend::default(),
                &MemorySnapshotReader::new(bytes),
            )
            .await
            .expect("completion transition should be fenced"),
        ContentUploadRunOutcome::Fenced
    );
}

#[tokio::test]
async fn missing_record_is_reported_without_touching_ports() {
    let store = MemoryContentStorageStore::default();
    let backend = ScriptedUploadBackend::default();
    let source = MemorySnapshotReader::new(Bytes::new());
    let operation =
        OperationId::new("missing-runner-operation").expect("operation fixture should be valid");
    assert_eq!(
        ContentUploadRunner::new(1)
            .expect("runner config should be valid")
            .resume(&operation, generation(1), &store, &backend, &source,)
            .await,
        Err(ContentUploadRunError::RecordNotFound)
    );
}

#[tokio::test]
async fn two_runner_invocations_converge_on_one_completed_record() {
    let fixture = SyntheticBackend::full();
    let bytes = Bytes::from_static(b"concurrent-runner");
    let intent = intent(&fixture, &bytes);
    let store = MemoryContentStorageStore::default();
    prepare_store(&store, &intent).await;
    store
        .persist_content_upload_intent(intent.clone(), generation(1))
        .await
        .expect("intent should persist");
    let source = MemorySnapshotReader::new(bytes);
    let backend = ScriptedUploadBackend::default();
    let runner = ContentUploadRunner::new(4).expect("runner config should be valid");

    let (first, second) = tokio::join!(
        runner.resume(
            intent.operation_id(),
            generation(1),
            &store,
            &backend,
            &source,
        ),
        runner.resume(
            intent.operation_id(),
            generation(1),
            &store,
            &backend,
            &source,
        )
    );
    assert!(matches!(
        first.expect("first runner should converge"),
        ContentUploadRunOutcome::Completed
    ));
    assert!(matches!(
        second.expect("second runner should converge"),
        ContentUploadRunOutcome::Completed
    ));
    assert_eq!(
        store
            .load_content_upload(intent.operation_id())
            .await
            .expect("upload load should succeed")
            .expect("upload record should exist")
            .state(),
        ContentUploadState::Completed
    );
}
