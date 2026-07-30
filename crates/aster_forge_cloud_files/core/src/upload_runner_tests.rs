use super::*;

use async_trait::async_trait;

use crate::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope, ContentRevision,
    LocalContentReference, RecoveryPage,
};

#[derive(Debug)]
struct StaticUploadStore {
    record: ContentUploadRecord,
}

#[async_trait]
impl ContentUploadStore for StaticUploadStore {
    async fn persist_content_upload_intent(
        &self,
        _intent: ContentUploadIntent,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted-record resume must not persist a new intent")
    }

    async fn load_content_upload(
        &self,
        _operation_id: &OperationId,
    ) -> StoreResult<Option<ContentUploadRecord>> {
        Ok(Some(self.record.clone()))
    }

    async fn recoverable_content_uploads_page(
        &self,
        _scope: &CloudScope,
        _after_operation_id: Option<&str>,
        _limit: usize,
    ) -> StoreResult<RecoveryPage<ContentUploadRecord>> {
        unreachable!("corrupted-record resume must not scan recovery pages")
    }

    async fn record_content_upload_session(
        &self,
        _operation_id: &OperationId,
        _session: ContentUploadSession,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted record must fail before recording a session")
    }

    async fn begin_content_upload_remote_commit(
        &self,
        _operation_id: &OperationId,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted record must fail before beginning remote commit")
    }

    async fn record_content_upload_remote_outcome(
        &self,
        _operation_id: &OperationId,
        _outcome: MutationRemoteOutcome,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted record must fail before recording a remote outcome")
    }

    async fn reconcile_content_upload_metadata(
        &self,
        _operation_id: &OperationId,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted record must fail before reconciling metadata")
    }

    async fn complete_content_upload(
        &self,
        _operation_id: &OperationId,
        _execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        unreachable!("corrupted record must fail before completing the upload")
    }
}

#[derive(Debug)]
struct UnusedUploadPorts;

#[async_trait]
impl CloudContentUploadBackend for UnusedUploadPorts {
    async fn start_upload(
        &self,
        _intent: &ContentUploadIntent,
    ) -> BackendResult<ContentUploadSession> {
        unreachable!("corrupted record must fail before starting backend upload")
    }

    async fn upload_chunk(
        &self,
        _intent: &ContentUploadIntent,
        _chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        unreachable!("corrupted record must fail before uploading a chunk")
    }

    async fn reconcile_upload_chunk(
        &self,
        _intent: &ContentUploadIntent,
        _session: &ContentUploadSession,
        _chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck> {
        unreachable!("corrupted record must fail before reconciling a chunk")
    }

    async fn commit_upload(
        &self,
        _intent: &ContentUploadIntent,
        _session: &ContentUploadSession,
    ) -> BackendResult<MutationRemoteOutcome> {
        unreachable!("corrupted record must fail before committing remote content")
    }

    async fn reconcile_upload(
        &self,
        _intent: &ContentUploadIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        unreachable!("corrupted record must fail before remote reconciliation")
    }
}

#[async_trait]
impl LocalContentSnapshotReader for UnusedUploadPorts {
    async fn read_snapshot(
        &self,
        _snapshot: &LocalContentSnapshot,
        _offset: u64,
        _length: u64,
    ) -> StoreResult<Bytes> {
        unreachable!("corrupted record must fail before reading snapshot bytes")
    }
}

fn intent() -> ContentUploadIntent {
    let item_key = CloudItemKey::new(
        CloudScope::new(
            CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
            CloudRootId::new("root").expect("root fixture should be valid"),
        ),
        CloudItemId::new("item").expect("item fixture should be valid"),
    );
    let snapshot = LocalContentSnapshot::new(
        item_key.clone(),
        LocalContentGeneration::new(1).expect("local generation fixture should be valid"),
        LocalContentReference::new("local-reference")
            .expect("local reference fixture should be valid"),
        4,
        None,
    );
    ContentUploadIntent::new(
        OperationId::new("operation").expect("operation fixture should be valid"),
        IdempotencyKey::new("idempotency").expect("idempotency fixture should be valid"),
        ContentCacheKey::new(
            item_key,
            ContentRevision::from_slice(b"content-v1")
                .expect("content revision fixture should be valid"),
        ),
        snapshot,
        ContentLeaseId::new("upload-lease").expect("lease fixture should be valid"),
        SessionGeneration::new(1).expect("generation fixture should be valid"),
    )
    .expect("upload intent fixture should be valid")
}

#[tokio::test]
async fn runner_rejects_corrupted_session_states_at_the_store_boundary() {
    let runner = ContentUploadRunner::new(4).expect("runner fixture should be valid");
    let ports = UnusedUploadPorts;

    for (state, reason) in [
        (
            ContentUploadState::Uploading,
            "uploading record omitted its backend session",
        ),
        (
            ContentUploadState::RemoteCommitting,
            "remote-committing record omitted its backend session",
        ),
    ] {
        let mut record = ContentUploadRecord::persist(intent());
        record.state = state;
        let operation_id = record.intent().operation_id().clone();
        let store = StaticUploadStore { record };

        assert_eq!(
            runner
                .resume(
                    &operation_id,
                    SessionGeneration::new(1).expect("generation fixture should be valid"),
                    &store,
                    &ports,
                    &ports,
                )
                .await,
            Err(ContentUploadRunError::Contract(
                CloudFilesCoreError::InvalidContentUploadTransition { reason }
            ))
        );
    }
}
