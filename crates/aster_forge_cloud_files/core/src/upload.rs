//! Product-neutral resumable content-upload requests, backend port, and durable recovery model.

use std::fmt;

use async_trait::async_trait;
use bytes::Bytes;

use crate::{
    BackendResult, CloudFilesCoreError, ContentCacheKey, ContentLeaseId, IdempotencyKey,
    LocalContentGeneration, LocalContentSnapshot, MutationRemoteOutcome, OperationId, Result,
    SessionGeneration,
};

/// Opaque backend upload-session identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentUploadSessionId(String);

impl ContentUploadSessionId {
    /// Creates a non-empty backend session identity.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(CloudFilesCoreError::empty("content upload session id"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque backend session identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContentUploadSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentUploadSessionId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Immutable upload intent persisted before acquiring or resuming a backend upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadIntent {
    operation_id: OperationId,
    idempotency_key: IdempotencyKey,
    base_cache_key: ContentCacheKey,
    snapshot: LocalContentSnapshot,
    upload_lease_id: ContentLeaseId,
    session_generation: SessionGeneration,
}

impl ContentUploadIntent {
    /// Creates an upload bound to one base remote revision and immutable local generation.
    pub fn new(
        operation_id: OperationId,
        idempotency_key: IdempotencyKey,
        base_cache_key: ContentCacheKey,
        snapshot: LocalContentSnapshot,
        upload_lease_id: ContentLeaseId,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        if base_cache_key.item_key() != snapshot.item_key() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload snapshot item does not match the base cache key",
            ));
        }
        Ok(Self {
            operation_id,
            idempotency_key,
            base_cache_key,
            snapshot,
            upload_lease_id,
            session_generation,
        })
    }

    /// Returns the durable operation identity shared with mutation reconciliation.
    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Returns the backend idempotency/status reconciliation key.
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Returns the exact remote content revision used as the conditional upload base.
    pub const fn base_cache_key(&self) -> &ContentCacheKey {
        &self.base_cache_key
    }

    /// Returns the immutable local source snapshot.
    pub const fn snapshot(&self) -> &LocalContentSnapshot {
        &self.snapshot
    }

    /// Returns the operation-owned upload lease identity.
    pub const fn upload_lease_id(&self) -> &ContentLeaseId {
        &self.upload_lease_id
    }

    /// Returns the platform session generation that accepted the upload.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Backend upload session and the next byte offset that remains to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadSession {
    id: ContentUploadSessionId,
    accepted_offset: u64,
}

impl ContentUploadSession {
    /// Creates a backend upload-session checkpoint.
    pub const fn new(id: ContentUploadSessionId, accepted_offset: u64) -> Self {
        Self {
            id,
            accepted_offset,
        }
    }

    /// Returns the backend upload-session identity.
    pub const fn id(&self) -> &ContentUploadSessionId {
        &self.id
    }

    /// Returns the first local byte not yet accepted by the backend.
    pub const fn accepted_offset(&self) -> u64 {
        self.accepted_offset
    }

    /// Validates that the backend checkpoint remains within the immutable snapshot.
    pub fn validate_for(&self, intent: &ContentUploadIntent) -> Result<()> {
        if self.accepted_offset > intent.snapshot().size() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "backend upload offset exceeds the immutable snapshot size",
            ));
        }
        Ok(())
    }
}

/// One immutable local byte chunk submitted to a backend upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadChunk {
    session_id: ContentUploadSessionId,
    local_generation: LocalContentGeneration,
    offset: u64,
    bytes: Bytes,
    byte_len: u64,
    total_size: u64,
}

impl ContentUploadChunk {
    /// Creates a non-empty chunk within the immutable snapshot size.
    pub fn new(
        session_id: ContentUploadSessionId,
        local_generation: LocalContentGeneration,
        offset: u64,
        bytes: Bytes,
        total_size: u64,
    ) -> Result<Self> {
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            CloudFilesCoreError::invalid_content_upload(
                "upload chunk length cannot be represented as u64",
            )
        })?;
        if byte_len == 0 {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunks must contain at least one byte",
            ));
        }
        let end = offset.checked_add(byte_len).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_upload("upload chunk end exceeds u64")
        })?;
        if end > total_size {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunk exceeds the immutable snapshot size",
            ));
        }
        Ok(Self {
            session_id,
            local_generation,
            offset,
            bytes,
            byte_len,
            total_size,
        })
    }

    /// Returns the backend upload-session identity.
    pub const fn session_id(&self) -> &ContentUploadSessionId {
        &self.session_id
    }

    /// Returns the immutable local generation whose bytes are carried.
    pub const fn local_generation(&self) -> LocalContentGeneration {
        self.local_generation
    }

    /// Returns the first byte offset in the immutable snapshot.
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the owned upload bytes.
    pub const fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Returns the number of bytes in this chunk.
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Returns the exclusive end acknowledged when this chunk commits exactly.
    pub const fn end_exclusive(&self) -> u64 {
        self.offset + self.byte_len
    }

    /// Returns the complete immutable snapshot size.
    pub const fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Validates that this chunk continues the exact durable session and immutable snapshot.
    pub fn validate_for(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
    ) -> Result<()> {
        if &self.session_id != session.id() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunk belongs to another backend session",
            ));
        }
        if self.local_generation != intent.snapshot().generation() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunk belongs to another local generation",
            ));
        }
        if self.total_size != intent.snapshot().size() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunk size does not match the immutable snapshot",
            ));
        }
        if self.offset != session.accepted_offset() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload chunk does not continue the durable accepted offset",
            ));
        }
        Ok(())
    }
}

/// Backend acknowledgement of one exact upload chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadChunkAck {
    session_id: ContentUploadSessionId,
    accepted_offset: u64,
}

impl ContentUploadChunkAck {
    /// Creates an acknowledgement whose offset is validated by the submitted chunk.
    pub const fn new(session_id: ContentUploadSessionId, accepted_offset: u64) -> Self {
        Self {
            session_id,
            accepted_offset,
        }
    }

    /// Returns the backend upload-session identity.
    pub const fn session_id(&self) -> &ContentUploadSessionId {
        &self.session_id
    }

    /// Returns the first byte not yet accepted by the backend.
    pub const fn accepted_offset(&self) -> u64 {
        self.accepted_offset
    }

    /// Validates exact, monotonic acknowledgement of the submitted chunk.
    pub fn validate_for(&self, chunk: &ContentUploadChunk) -> Result<()> {
        if self.session_id != chunk.session_id {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload acknowledgement belongs to another session",
            ));
        }
        if self.accepted_offset != chunk.end_exclusive() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload acknowledgement does not match the submitted chunk end",
            ));
        }
        Ok(())
    }
}

/// Product-owned resumable upload transport.
#[async_trait]
pub trait CloudContentUploadBackend: Send + Sync {
    /// Starts or idempotently resumes a backend session for one immutable upload intent.
    async fn start_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<ContentUploadSession>;

    /// Uploads one exact immutable chunk.
    async fn upload_chunk(
        &self,
        intent: &ContentUploadIntent,
        chunk: &ContentUploadChunk,
    ) -> BackendResult<ContentUploadChunkAck>;

    /// Conditionally commits the fully uploaded content and returns a mutation outcome.
    async fn commit_upload(
        &self,
        intent: &ContentUploadIntent,
        session: &ContentUploadSession,
    ) -> BackendResult<MutationRemoteOutcome>;

    /// Reconciles a commit whose transport outcome did not prove whether it succeeded.
    async fn reconcile_upload(
        &self,
        intent: &ContentUploadIntent,
    ) -> BackendResult<MutationRemoteOutcome>;
}

/// Durable upload state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentUploadState {
    /// Intent, dirty generation, and upload lease are durable.
    IntentPersisted,
    /// A backend session and resumable accepted offset are durable.
    Uploading,
    /// Every byte was accepted and remote commit has started.
    RemoteCommitting,
    /// Transport completion did not prove whether remote commit happened.
    RemoteOutcomeUnknown,
    /// A committed, already-committed, or precondition-failed outcome is durable.
    RemoteOutcomeKnown,
    /// Local metadata acknowledged the outcome without clearing a newer dirty generation.
    MetadataReconciled,
    /// The operation is terminal and its upload lease was released.
    Completed,
}

/// Result of an idempotent upload-record transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentUploadRecordTransition {
    /// Durable state changed.
    Applied,
    /// Equivalent or later durable state already contains the transition.
    AlreadyApplied,
    /// A newer durable offset or local generation superseded the transition.
    Fenced,
}

/// Recoverable resumable upload journal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadRecord {
    intent: ContentUploadIntent,
    state: ContentUploadState,
    session: Option<ContentUploadSession>,
    remote_outcome: Option<MutationRemoteOutcome>,
}

impl ContentUploadRecord {
    /// Creates the first recoverable upload state.
    pub const fn persist(intent: ContentUploadIntent) -> Self {
        Self {
            intent,
            state: ContentUploadState::IntentPersisted,
            session: None,
            remote_outcome: None,
        }
    }

    /// Returns the immutable upload intent.
    pub const fn intent(&self) -> &ContentUploadIntent {
        &self.intent
    }

    /// Returns the durable upload state.
    pub const fn state(&self) -> ContentUploadState {
        self.state
    }

    /// Returns the durable backend session checkpoint.
    pub const fn session(&self) -> Option<&ContentUploadSession> {
        self.session.as_ref()
    }

    /// Returns the durable remote outcome.
    pub const fn remote_outcome(&self) -> Option<&MutationRemoteOutcome> {
        self.remote_outcome.as_ref()
    }

    /// Records or advances the backend session's accepted offset.
    pub fn record_session(
        &mut self,
        session: ContentUploadSession,
    ) -> Result<ContentUploadRecordTransition> {
        session.validate_for(&self.intent)?;
        if !matches!(
            self.state,
            ContentUploadState::IntentPersisted | ContentUploadState::Uploading
        ) {
            if self.session.as_ref() == Some(&session) {
                return Ok(ContentUploadRecordTransition::AlreadyApplied);
            }
            return Err(CloudFilesCoreError::invalid_content_upload(
                "upload session checkpoint must precede remote commit",
            ));
        }
        let Some(current) = self.session.as_ref() else {
            self.state = ContentUploadState::Uploading;
            self.session = Some(session);
            return Ok(ContentUploadRecordTransition::Applied);
        };
        if current.id() != session.id() {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "one upload operation cannot switch backend session identity",
            ));
        }
        if session.accepted_offset() < current.accepted_offset() {
            return Ok(ContentUploadRecordTransition::Fenced);
        }
        if session.accepted_offset() == current.accepted_offset() {
            return Ok(ContentUploadRecordTransition::AlreadyApplied);
        }
        self.session = Some(session);
        Ok(ContentUploadRecordTransition::Applied)
    }

    /// Marks remote commit started after all immutable bytes were accepted.
    pub fn begin_remote_commit(&mut self) -> Result<ContentUploadRecordTransition> {
        match self.state {
            ContentUploadState::Uploading => {
                let complete = self.session.as_ref().is_some_and(|session| {
                    session.accepted_offset() == self.intent.snapshot().size()
                });
                if !complete {
                    return Err(CloudFilesCoreError::invalid_content_upload(
                        "remote commit requires every immutable byte to be accepted",
                    ));
                }
                self.state = ContentUploadState::RemoteCommitting;
                Ok(ContentUploadRecordTransition::Applied)
            }
            ContentUploadState::RemoteCommitting
            | ContentUploadState::RemoteOutcomeUnknown
            | ContentUploadState::RemoteOutcomeKnown
            | ContentUploadState::MetadataReconciled
            | ContentUploadState::Completed => Ok(ContentUploadRecordTransition::AlreadyApplied),
            ContentUploadState::IntentPersisted => {
                Err(CloudFilesCoreError::invalid_content_upload(
                    "remote commit requires a backend upload session",
                ))
            }
        }
    }

    /// Records a known or unknown conditional remote outcome.
    pub fn record_remote_outcome(
        &mut self,
        outcome: MutationRemoteOutcome,
    ) -> Result<ContentUploadRecordTransition> {
        if let MutationRemoteOutcome::Committed { item }
        | MutationRemoteOutcome::AlreadyCommitted { item } = &outcome
        {
            let Some(item) = item.as_ref() else {
                return Err(CloudFilesCoreError::invalid_content_upload(
                    "committed upload outcome requires current item metadata",
                ));
            };
            if item.key() != self.intent.base_cache_key().item_key() {
                return Err(CloudFilesCoreError::invalid_content_upload(
                    "committed upload outcome belongs to another item",
                ));
            }
            let Some(content) = item.content() else {
                return Err(CloudFilesCoreError::invalid_content_upload(
                    "committed upload outcome must describe file content",
                ));
            };
            if content.size() != self.intent.snapshot().size() {
                return Err(CloudFilesCoreError::invalid_content_upload(
                    "committed upload size does not match the immutable snapshot",
                ));
            }
        }
        let next_state = if matches!(outcome, MutationRemoteOutcome::RemoteOutcomeUnknown) {
            ContentUploadState::RemoteOutcomeUnknown
        } else {
            ContentUploadState::RemoteOutcomeKnown
        };
        if self.remote_outcome.as_ref() == Some(&outcome)
            && matches!(
                self.state,
                ContentUploadState::RemoteOutcomeUnknown
                    | ContentUploadState::RemoteOutcomeKnown
                    | ContentUploadState::MetadataReconciled
                    | ContentUploadState::Completed
            )
        {
            return Ok(ContentUploadRecordTransition::AlreadyApplied);
        }
        if !matches!(
            self.state,
            ContentUploadState::RemoteCommitting | ContentUploadState::RemoteOutcomeUnknown
        ) {
            return Err(CloudFilesCoreError::invalid_content_upload(
                "remote upload outcome must follow commit or reconcile an unknown outcome",
            ));
        }
        self.state = next_state;
        self.remote_outcome = Some(outcome);
        Ok(ContentUploadRecordTransition::Applied)
    }

    /// Marks local metadata reconciled after a known remote outcome.
    pub fn mark_metadata_reconciled(&mut self) -> Result<ContentUploadRecordTransition> {
        match self.state {
            ContentUploadState::RemoteOutcomeKnown => {
                self.state = ContentUploadState::MetadataReconciled;
                Ok(ContentUploadRecordTransition::Applied)
            }
            ContentUploadState::MetadataReconciled | ContentUploadState::Completed => {
                Ok(ContentUploadRecordTransition::AlreadyApplied)
            }
            _ => Err(CloudFilesCoreError::invalid_content_upload(
                "upload metadata reconciliation requires a known remote outcome",
            )),
        }
    }

    /// Marks the upload terminal after metadata reconciliation.
    pub fn complete(&mut self) -> Result<ContentUploadRecordTransition> {
        match self.state {
            ContentUploadState::MetadataReconciled => {
                self.state = ContentUploadState::Completed;
                Ok(ContentUploadRecordTransition::Applied)
            }
            ContentUploadState::Completed => Ok(ContentUploadRecordTransition::AlreadyApplied),
            _ => Err(CloudFilesCoreError::invalid_content_upload(
                "upload completion requires metadata reconciliation",
            )),
        }
    }
}
