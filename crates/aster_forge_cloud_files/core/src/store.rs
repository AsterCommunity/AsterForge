//! Product-neutral durable-store ports for checkpoints, mutations, cache writes, uploads, and eviction.

use std::fmt;

use async_trait::async_trait;

use crate::{
    ByteRange, ChangeBatch, ChangeCursor, CloudScope, ContentCacheKey, ContentCacheWriteIntent,
    ContentCacheWriteOperationId, ContentCacheWriteRecord, ContentEvictionBegin,
    ContentEvictionIntent, ContentEvictionOperationId, ContentEvictionPhysicalEffect,
    ContentEvictionRecord, ContentLeaseId, ContentLeaseKind, ContentStorageEntry,
    ContentUploadIntent, ContentUploadRecord, ContentUploadSession, LocalContentGeneration,
    LocalContentSnapshot, MutationIntent, MutationRecord, MutationRemoteOutcome, OperationId,
    PlatformMaterializationState, SessionGeneration, SessionState,
};

/// Result returned by cloud-files durable-store ports.
pub type StoreResult<T> = std::result::Result<T, CloudFilesStoreError>;

/// Stable classification of a persistence-layer failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudFilesStoreErrorKind {
    /// A requested checkpoint, batch, session, content entry, or operation record does not exist.
    NotFound,
    /// Existing durable state conflicts with the requested write.
    Conflict,
    /// The requested state transition violates the durable protocol.
    InvalidTransition,
    /// The persistence implementation did not durably complete the requested operation.
    PersistenceFailure,
}

/// Product-neutral store failure with adapter-owned diagnostic context.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cloud-files store {kind:?}: {context}")]
pub struct CloudFilesStoreError {
    kind: CloudFilesStoreErrorKind,
    context: String,
}

impl CloudFilesStoreError {
    /// Creates a classified persistence error.
    pub fn new(kind: CloudFilesStoreErrorKind, context: impl Into<String>) -> Self {
        Self {
            kind,
            context: context.into(),
        }
    }

    /// Returns the stable error classification.
    pub const fn kind(&self) -> CloudFilesStoreErrorKind {
        self.kind
    }

    /// Returns implementation diagnostic context. Product layers map user-visible text.
    pub fn context(&self) -> &str {
        &self.context
    }
}

/// Stable identity of one persisted change batch.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ChangeBatchId(String);

impl ChangeBatchId {
    /// Creates a non-empty batch identity.
    pub fn new(value: impl Into<String>) -> crate::Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(crate::CloudFilesCoreError::empty("change batch id"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque batch identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ChangeBatchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChangeBatchId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Durable state of one pending change batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersistedChangeBatchState {
    /// Batch bytes and base checkpoint are durable, but effects may not yet be replayable.
    Recorded,
    /// All effects needed after restart are durable and may be replayed idempotently.
    EffectsReplayable,
}

/// Change batch persisted before its following cursor becomes active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedChangeBatch {
    id: ChangeBatchId,
    scope: CloudScope,
    base_cursor: Option<ChangeCursor>,
    batch: ChangeBatch,
    state: PersistedChangeBatchState,
}

impl PersistedChangeBatch {
    /// Creates the first durable form of a fetched change batch.
    pub const fn new(
        id: ChangeBatchId,
        scope: CloudScope,
        base_cursor: Option<ChangeCursor>,
        batch: ChangeBatch,
    ) -> Self {
        Self {
            id,
            scope,
            base_cursor,
            batch,
            state: PersistedChangeBatchState::Recorded,
        }
    }

    /// Returns the batch identity.
    pub const fn id(&self) -> &ChangeBatchId {
        &self.id
    }

    /// Returns the namespace/root checkpoint scope.
    pub const fn scope(&self) -> &CloudScope {
        &self.scope
    }

    /// Returns the active cursor from which this batch was fetched.
    pub const fn base_cursor(&self) -> Option<&ChangeCursor> {
        self.base_cursor.as_ref()
    }

    /// Returns the durable backend batch.
    pub const fn batch(&self) -> &ChangeBatch {
        &self.batch
    }

    /// Returns whether replay prerequisites are durable.
    pub const fn state(&self) -> PersistedChangeBatchState {
        self.state
    }

    /// Marks the batch effects replayable. The operation is idempotent.
    pub fn mark_effects_replayable(&mut self) -> bool {
        if self.state == PersistedChangeBatchState::EffectsReplayable {
            return false;
        }
        self.state = PersistedChangeBatchState::EffectsReplayable;
        true
    }
}

/// Durable checkpoint snapshot for one namespace/root scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeCursorCheckpoint {
    scope: CloudScope,
    active_cursor: Option<ChangeCursor>,
    pending_batch: Option<PersistedChangeBatch>,
}

impl ChangeCursorCheckpoint {
    /// Creates an empty checkpoint at the initial backend position.
    pub const fn initial(scope: CloudScope) -> Self {
        Self {
            scope,
            active_cursor: None,
            pending_batch: None,
        }
    }

    /// Creates a checkpoint snapshot returned by a store implementation.
    pub const fn new(
        scope: CloudScope,
        active_cursor: Option<ChangeCursor>,
        pending_batch: Option<PersistedChangeBatch>,
    ) -> Self {
        Self {
            scope,
            active_cursor,
            pending_batch,
        }
    }

    /// Returns the namespace/root scope.
    pub const fn scope(&self) -> &CloudScope {
        &self.scope
    }

    /// Returns the cursor that is safe to use for the next backend fetch.
    pub const fn active_cursor(&self) -> Option<&ChangeCursor> {
        self.active_cursor.as_ref()
    }

    /// Returns the batch that must be replayed or completed before another fetch.
    pub const fn pending_batch(&self) -> Option<&PersistedChangeBatch> {
        self.pending_batch.as_ref()
    }
}

/// Result of an idempotent store write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreWriteStatus {
    /// Durable state changed.
    Applied,
    /// Equivalent durable state already existed.
    AlreadyApplied,
    /// The active session generation or lifecycle fence rejected this write.
    Fenced,
}

/// Durable two-phase change-checkpoint protocol.
#[async_trait]
pub trait ChangeCheckpointStore: Send + Sync {
    /// Loads the active cursor and any pending replayable batch for one scope.
    async fn load_change_checkpoint(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<ChangeCursorCheckpoint>;

    /// Persists a batch against the current active cursor without advancing that cursor.
    async fn record_change_batch(
        &self,
        batch: PersistedChangeBatch,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks all post-crash effects for the pending batch durably replayable.
    async fn mark_change_effects_replayable(
        &self,
        scope: &CloudScope,
        batch_id: &ChangeBatchId,
    ) -> StoreResult<StoreWriteStatus>;

    /// Atomically promotes a replayable batch cursor and clears its pending slot.
    ///
    /// Repeating a successful commit for the same batch identity must be idempotent.
    async fn commit_change_cursor(
        &self,
        scope: &CloudScope,
        batch_id: &ChangeBatchId,
    ) -> StoreResult<StoreWriteStatus>;
}

/// Durable content metadata, runtime lease, and eviction-recovery protocol.
///
/// Implementations may compose a database-backed metadata repository with an in-process lease
/// registry. The port requires their eviction reservation decision to be serialized so a lease
/// cannot start between guard evaluation and intent persistence.
#[async_trait]
pub trait ContentStorageStore: Send + Sync {
    /// Creates a revision-bound content entry without replacing a different existing snapshot.
    async fn create_content_entry(
        &self,
        entry: ContentStorageEntry,
    ) -> StoreResult<StoreWriteStatus>;

    /// Loads one content storage snapshot including current runtime lease counts.
    async fn load_content_entry(
        &self,
        key: &ContentCacheKey,
    ) -> StoreResult<Option<ContentStorageEntry>>;

    /// Records provider-owned cached range coverage.
    async fn record_provider_cached_range(
        &self,
        key: &ContentCacheKey,
        range: ByteRange,
    ) -> StoreResult<StoreWriteStatus>;

    /// Records platform-observed materialization without writing provider-cache coverage.
    async fn observe_platform_materialization(
        &self,
        key: &ContentCacheKey,
        state: PlatformMaterializationState,
    ) -> StoreResult<StoreWriteStatus>;

    /// Updates the pin guard.
    async fn set_content_pinned(
        &self,
        key: &ContentCacheKey,
        pinned: bool,
    ) -> StoreResult<StoreWriteStatus>;

    /// Records an immutable dirty snapshot, fencing an older local generation.
    async fn mark_content_dirty(
        &self,
        key: &ContentCacheKey,
        snapshot: LocalContentSnapshot,
    ) -> StoreResult<StoreWriteStatus>;

    /// Clears dirty state only if the completed upload still owns the active generation.
    async fn clear_content_dirty(
        &self,
        key: &ContentCacheKey,
        generation: LocalContentGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Acquires an idempotently identified runtime lease, unless eviction already reserved entry.
    async fn acquire_content_lease(
        &self,
        key: &ContentCacheKey,
        lease_id: ContentLeaseId,
        kind: ContentLeaseKind,
    ) -> StoreResult<StoreWriteStatus>;

    /// Releases one runtime lease.
    async fn release_content_lease(
        &self,
        key: &ContentCacheKey,
        lease_id: &ContentLeaseId,
    ) -> StoreResult<StoreWriteStatus>;

    /// Atomically checks guards, reserves the entry, and persists an eviction intent.
    async fn begin_content_eviction(
        &self,
        intent: ContentEvictionIntent,
    ) -> StoreResult<ContentEvictionBegin>;

    /// Loads one durable eviction record.
    async fn load_content_eviction(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<Option<ContentEvictionRecord>>;

    /// Lists non-completed eviction records for startup reconciliation.
    async fn recoverable_content_evictions(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentEvictionRecord>>;

    /// Records one idempotent physical effect observed by cache/platform reconciliation.
    async fn record_content_eviction_effect(
        &self,
        operation_id: &ContentEvictionOperationId,
        effect: ContentEvictionPhysicalEffect,
    ) -> StoreResult<StoreWriteStatus>;

    /// Atomically applies the physical result to content metadata.
    async fn reconcile_content_eviction_metadata(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks the eviction terminal and releases its entry reservation.
    async fn complete_content_eviction(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<StoreWriteStatus>;
}

/// Durable provider-cache byte installation and sparse-coverage reconciliation protocol.
///
/// Implementations atomically reserve the target entry with intent persistence, and atomically
/// update provider coverage with the durable record's coverage transition.
#[async_trait]
pub trait ContentCacheWriteStore: Send + Sync {
    /// Atomically reserves the entry and persists a write intent before physical bytes are changed.
    async fn begin_content_cache_write(
        &self,
        intent: ContentCacheWriteIntent,
    ) -> StoreResult<StoreWriteStatus>;

    /// Loads one durable cache-write record.
    async fn load_content_cache_write(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<Option<ContentCacheWriteRecord>>;

    /// Lists non-completed cache writes for startup physical-state observation.
    async fn recoverable_content_cache_writes(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentCacheWriteRecord>>;

    /// Records that the intended bytes are physically committed and observable.
    async fn record_content_cache_write_physical_commit(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus>;

    /// Atomically records provider range coverage for the committed physical bytes.
    async fn reconcile_content_cache_write_coverage(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks the write terminal and releases its entry reservation.
    async fn complete_content_cache_write(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus>;
}

/// Durable resumable-upload checkpoint and dirty-generation reconciliation protocol.
///
/// Intent persistence atomically validates the active immutable dirty snapshot and acquires its
/// operation-owned upload lease. Metadata reconciliation clears dirty state only when the upload's
/// local generation is still current; a newer generation fences stale completion. Every mutating
/// transition also receives the executor's platform session generation. Implementations must
/// compare it with the active generation in the same transaction as the requested transition.
#[async_trait]
pub trait ContentUploadStore: Send + Sync {
    /// Atomically validates dirty state and generation, acquires the lease, and persists intent.
    async fn persist_content_upload_intent(
        &self,
        intent: ContentUploadIntent,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Loads one durable upload record.
    async fn load_content_upload(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<ContentUploadRecord>>;

    /// Lists non-completed upload records for startup resume or outcome reconciliation.
    async fn recoverable_content_uploads(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentUploadRecord>>;

    /// Records a backend session or monotonically advances its accepted offset when unfenced.
    async fn record_content_upload_session(
        &self,
        operation_id: &OperationId,
        session: ContentUploadSession,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks remote commit started after all immutable bytes were accepted when unfenced.
    async fn begin_content_upload_remote_commit(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Records a known or unknown remote upload outcome when the executor remains active.
    async fn record_content_upload_remote_outcome(
        &self,
        operation_id: &OperationId,
        outcome: MutationRemoteOutcome,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Reconciles the outcome and conditionally clears dirty state when the executor is active.
    async fn reconcile_content_upload_metadata(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks the upload terminal and releases its lease when the executor remains active.
    async fn complete_content_upload(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;
}

/// Durable mutation journal and active-session fence protocol.
#[async_trait]
pub trait MutationJournalStore: Send + Sync {
    /// Activates a generation in `Accepting`, fencing all lower generations for the same scope.
    async fn activate_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Loads the active generation and lifecycle state for one scope.
    async fn load_session(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Option<(SessionGeneration, SessionState)>>;

    /// Advances one active generation through closing, draining, and closed.
    async fn transition_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
        next: SessionState,
    ) -> StoreResult<StoreWriteStatus>;

    /// Inserts the first recoverable mutation state before ingress acknowledgement.
    async fn persist_mutation_intent(
        &self,
        intent: MutationIntent,
    ) -> StoreResult<StoreWriteStatus>;

    /// Loads one durable mutation record.
    async fn load_mutation(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<MutationRecord>>;

    /// Lists non-completed records for deterministic startup recovery.
    async fn recoverable_mutations(&self, scope: &CloudScope) -> StoreResult<Vec<MutationRecord>>;

    /// Marks remote application started under the active generation.
    ///
    /// A newer generation may resume an older persisted intent; a completion still carrying an
    /// inactive generation is fenced.
    async fn begin_remote_apply(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Records a remote outcome or reconciles a previous unknown outcome under the active session.
    async fn record_remote_outcome(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
        outcome: MutationRemoteOutcome,
    ) -> StoreResult<StoreWriteStatus>;

    /// Durably reconciles required product/local platform state for a still-active generation.
    ///
    /// Implementations must generation-check and complete, or durably enqueue, the required local
    /// metadata, namespace mapping, and platform reconciliation in the same transaction as this
    /// marker. This method must not write a marker that falsely claims reconciliation while the
    /// corresponding product/platform effect remains neither durable nor replayable.
    async fn mark_platform_reconciled(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;

    /// Marks a reconciled mutation terminal without repeating its remote effect.
    async fn complete_mutation(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus>;
}
