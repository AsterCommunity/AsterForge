use std::{collections::HashMap, sync::Mutex};

use aster_forge_cloud_files_core::{
    ByteRange, CloudFilesCoreError, CloudFilesStoreError, CloudFilesStoreErrorKind, CloudScope,
    ContentCacheKey, ContentCacheWriteExtent, ContentCacheWriteIntent,
    ContentCacheWriteOperationId, ContentCacheWriteRecord, ContentCacheWriteRecordTransition,
    ContentCacheWriteState, ContentCacheWriteStore, ContentEvictionBegin, ContentEvictionDecision,
    ContentEvictionIntent, ContentEvictionOperationId, ContentEvictionPhysicalEffect,
    ContentEvictionRecord, ContentEvictionRecordTransition, ContentEvictionState, ContentLeaseId,
    ContentLeaseKind, ContentStorageEntry, ContentStorageStore, ContentUploadIntent,
    ContentUploadRecord, ContentUploadRecordTransition, ContentUploadSession, ContentUploadState,
    ContentUploadStore, DirtyContentTransition, LocalContentGeneration, LocalContentSnapshot,
    MutationRemoteOutcome, OperationId, PlatformMaterializationState, StoreResult,
    StoreWriteStatus,
};
use async_trait::async_trait;

use super::failure_injection::{FailureInjector, FailurePoint};

#[derive(Debug, Default)]
struct ContentStorageState {
    entries: HashMap<ContentCacheKey, ContentStorageEntry>,
    cache_writes: HashMap<ContentCacheWriteOperationId, ContentCacheWriteRecord>,
    evictions: HashMap<ContentEvictionOperationId, ContentEvictionRecord>,
    uploads: HashMap<OperationId, ContentUploadRecord>,
}

#[derive(Debug, Default)]
pub struct MemoryContentStorageStore {
    state: Mutex<ContentStorageState>,
    failures: FailureInjector,
}

impl MemoryContentStorageStore {
    pub fn failures(&self) -> &FailureInjector {
        &self.failures
    }

    fn store_error(
        kind: CloudFilesStoreErrorKind,
        context: impl Into<String>,
    ) -> CloudFilesStoreError {
        CloudFilesStoreError::new(kind, context)
    }

    fn transition_error(error: CloudFilesCoreError) -> CloudFilesStoreError {
        Self::store_error(
            CloudFilesStoreErrorKind::InvalidTransition,
            error.to_string(),
        )
    }

    fn write_status(changed: bool) -> StoreWriteStatus {
        if changed {
            StoreWriteStatus::Applied
        } else {
            StoreWriteStatus::AlreadyApplied
        }
    }

    fn transition_status(transition: ContentEvictionRecordTransition) -> StoreWriteStatus {
        match transition {
            ContentEvictionRecordTransition::Applied => StoreWriteStatus::Applied,
            ContentEvictionRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
        }
    }

    fn cache_write_transition_status(
        transition: ContentCacheWriteRecordTransition,
    ) -> StoreWriteStatus {
        match transition {
            ContentCacheWriteRecordTransition::Applied => StoreWriteStatus::Applied,
            ContentCacheWriteRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
        }
    }

    fn dirty_transition_status(transition: DirtyContentTransition) -> StoreWriteStatus {
        match transition {
            DirtyContentTransition::Applied => StoreWriteStatus::Applied,
            DirtyContentTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
            DirtyContentTransition::Fenced => StoreWriteStatus::Fenced,
        }
    }

    fn upload_transition_status(transition: ContentUploadRecordTransition) -> StoreWriteStatus {
        match transition {
            ContentUploadRecordTransition::Applied => StoreWriteStatus::Applied,
            ContentUploadRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
            ContentUploadRecordTransition::Fenced => StoreWriteStatus::Fenced,
        }
    }

    fn entry_mut<'a>(
        state: &'a mut ContentStorageState,
        key: &ContentCacheKey,
    ) -> StoreResult<&'a mut ContentStorageEntry> {
        state.entries.get_mut(key).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content storage entry was not found",
            )
        })
    }
}

#[async_trait]
impl ContentUploadStore for MemoryContentStorageStore {
    async fn persist_content_upload_intent(
        &self,
        intent: ContentUploadIntent,
    ) -> StoreResult<StoreWriteStatus> {
        let operation_id = intent.operation_id().clone();
        let cache_key = intent.base_cache_key().clone();
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        if let Some(existing) = state.uploads.get(&operation_id) {
            return if existing.intent() == &intent {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "content upload operation id already identifies another intent",
                ))
            };
        }
        if state.uploads.values().any(|record| {
            record.intent().idempotency_key() == intent.idempotency_key()
                || record.intent().upload_lease_id() == intent.upload_lease_id()
        }) {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "content upload idempotency or lease identity is already in use",
            ));
        }
        let entry = Self::entry_mut(&mut state, &cache_key)?;
        let Some(dirty) = entry.dirty_snapshot() else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "content upload requires an active dirty snapshot",
            ));
        };
        if dirty.generation() > intent.snapshot().generation() {
            return Ok(StoreWriteStatus::Fenced);
        }
        if dirty != intent.snapshot() {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "content upload snapshot does not match active dirty state",
            ));
        }
        entry
            .acquire_lease(intent.upload_lease_id().clone(), ContentLeaseKind::Upload)
            .map_err(Self::transition_error)?;
        state
            .uploads
            .insert(operation_id, ContentUploadRecord::persist(intent));
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentUploadIntentPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_content_upload(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<ContentUploadRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        Ok(state.uploads.get(operation_id).cloned())
    }

    async fn recoverable_content_uploads(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentUploadRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let mut records = state
            .uploads
            .values()
            .filter(|record| {
                record.intent().base_cache_key().item_key().scope() == scope
                    && record.state() != ContentUploadState::Completed
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.intent()
                .operation_id()
                .as_str()
                .cmp(right.intent().operation_id().as_str())
        });
        Ok(records)
    }

    async fn record_content_upload_session(
        &self,
        operation_id: &OperationId,
        session: ContentUploadSession,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.uploads.get_mut(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content upload operation was not found",
            )
        })?;
        let transition = record
            .record_session(session)
            .map_err(Self::transition_error)?;
        let status = Self::upload_transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentUploadCheckpointPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn begin_content_upload_remote_commit(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.uploads.get_mut(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content upload operation was not found",
            )
        })?;
        let transition = record
            .begin_remote_commit()
            .map_err(Self::transition_error)?;
        let status = Self::upload_transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentUploadRemoteCommitPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn record_content_upload_remote_outcome(
        &self,
        operation_id: &OperationId,
        outcome: MutationRemoteOutcome,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.uploads.get_mut(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content upload operation was not found",
            )
        })?;
        let transition = record
            .record_remote_outcome(outcome)
            .map_err(Self::transition_error)?;
        let status = Self::upload_transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentUploadOutcomePersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn reconcile_content_upload_metadata(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.uploads.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content upload operation was not found",
            )
        })?;
        if matches!(
            record.state(),
            ContentUploadState::MetadataReconciled | ContentUploadState::Completed
        ) {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let cache_key = record.intent().base_cache_key().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content entry for upload was not found",
            )
        })?;
        next_record
            .mark_metadata_reconciled()
            .map_err(Self::transition_error)?;
        let status = match next_record.remote_outcome() {
            Some(
                MutationRemoteOutcome::Committed { .. }
                | MutationRemoteOutcome::AlreadyCommitted { .. },
            ) => Self::dirty_transition_status(
                next_entry
                    .clear_dirty(next_record.intent().snapshot().generation())
                    .map_err(Self::transition_error)?,
            ),
            Some(MutationRemoteOutcome::PreconditionFailed { .. }) => StoreWriteStatus::Applied,
            Some(MutationRemoteOutcome::RemoteOutcomeUnknown) | None => {
                return Err(Self::store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "content upload metadata requires a known remote outcome",
                ));
            }
        };
        state.entries.insert(cache_key, next_entry);
        state.uploads.insert(operation_id.clone(), next_record);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentUploadMetadataPersistBeforeReturn)?;
        Ok(status)
    }

    async fn complete_content_upload(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.uploads.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content upload operation was not found",
            )
        })?;
        if record.state() == ContentUploadState::Completed {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let cache_key = record.intent().base_cache_key().clone();
        let lease_id = record.intent().upload_lease_id().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content entry for upload was not found",
            )
        })?;
        next_record.complete().map_err(Self::transition_error)?;
        next_entry
            .release_lease(&lease_id)
            .map_err(Self::transition_error)?;
        state.entries.insert(cache_key, next_entry);
        state.uploads.insert(operation_id.clone(), next_record);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentUploadCompletionPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }
}

#[async_trait]
impl ContentCacheWriteStore for MemoryContentStorageStore {
    async fn begin_content_cache_write(
        &self,
        intent: ContentCacheWriteIntent,
    ) -> StoreResult<StoreWriteStatus> {
        let operation_id = intent.operation_id().clone();
        let cache_key = intent.cache_key().clone();
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        if let Some(existing) = state.cache_writes.get(&operation_id) {
            return if existing.intent() == &intent {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "content cache write operation id already identifies another intent",
                ))
            };
        }
        let entry = Self::entry_mut(&mut state, &cache_key)?;
        if entry.total_size() != intent.expected_size() {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "content cache write size does not match the target entry",
            ));
        }
        entry.begin_cache_write().map_err(Self::transition_error)?;
        state
            .cache_writes
            .insert(operation_id, ContentCacheWriteRecord::persist(intent));
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentCacheWriteIntentPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_content_cache_write(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<Option<ContentCacheWriteRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        Ok(state.cache_writes.get(operation_id).cloned())
    }

    async fn recoverable_content_cache_writes(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentCacheWriteRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let mut records = state
            .cache_writes
            .values()
            .filter(|record| {
                record.intent().cache_key().item_key().scope() == scope
                    && record.state() != ContentCacheWriteState::Completed
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.intent()
                .operation_id()
                .as_str()
                .cmp(right.intent().operation_id().as_str())
        });
        Ok(records)
    }

    async fn record_content_cache_write_physical_commit(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.cache_writes.get_mut(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content cache write operation was not found",
            )
        })?;
        let transition = record
            .mark_physical_bytes_committed()
            .map_err(Self::transition_error)?;
        let status = Self::cache_write_transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentCacheWritePhysicalPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn reconcile_content_cache_write_coverage(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.cache_writes.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content cache write operation was not found",
            )
        })?;
        if matches!(
            record.state(),
            ContentCacheWriteState::CoverageCommitted | ContentCacheWriteState::Completed
        ) {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let cache_key = record.intent().cache_key().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content entry for cache write was not found",
            )
        })?;
        next_record
            .mark_coverage_committed()
            .map_err(Self::transition_error)?;
        match next_record.intent().extent() {
            ContentCacheWriteExtent::Whole if next_record.intent().expected_size() == 0 => {}
            ContentCacheWriteExtent::Whole => {
                let whole = ByteRange::new(0, next_record.intent().expected_size())
                    .map_err(Self::transition_error)?;
                next_entry
                    .record_provider_range(whole)
                    .map_err(Self::transition_error)?;
            }
            ContentCacheWriteExtent::Range(range) => {
                next_entry
                    .record_provider_range(range)
                    .map_err(Self::transition_error)?;
            }
        }
        state.entries.insert(cache_key, next_entry);
        state.cache_writes.insert(operation_id.clone(), next_record);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentCacheWriteCoveragePersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }

    async fn complete_content_cache_write(
        &self,
        operation_id: &ContentCacheWriteOperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.cache_writes.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content cache write operation was not found",
            )
        })?;
        if record.state() == ContentCacheWriteState::Completed {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let cache_key = record.intent().cache_key().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content entry for cache write was not found",
            )
        })?;
        next_record.complete().map_err(Self::transition_error)?;
        next_entry
            .complete_cache_write()
            .map_err(Self::transition_error)?;
        state.entries.insert(cache_key, next_entry);
        state.cache_writes.insert(operation_id.clone(), next_record);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentCacheWriteCompletionPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }
}

#[async_trait]
impl ContentStorageStore for MemoryContentStorageStore {
    async fn create_content_entry(
        &self,
        entry: ContentStorageEntry,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        if let Some(existing) = state.entries.get(entry.key()) {
            return if existing == &entry {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "content cache key already identifies a different entry",
                ))
            };
        }
        state.entries.insert(entry.key().clone(), entry);
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_content_entry(
        &self,
        key: &ContentCacheKey,
    ) -> StoreResult<Option<ContentStorageEntry>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        Ok(state.entries.get(key).cloned())
    }

    async fn record_provider_cached_range(
        &self,
        key: &ContentCacheKey,
        range: ByteRange,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let changed = Self::entry_mut(&mut state, key)?
            .record_provider_range(range)
            .map_err(Self::transition_error)?;
        Ok(Self::write_status(changed))
    }

    async fn observe_platform_materialization(
        &self,
        key: &ContentCacheKey,
        materialization: PlatformMaterializationState,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let changed = Self::entry_mut(&mut state, key)?
            .observe_platform_materialization(materialization)
            .map_err(Self::transition_error)?;
        Ok(Self::write_status(changed))
    }

    async fn set_content_pinned(
        &self,
        key: &ContentCacheKey,
        pinned: bool,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let changed = Self::entry_mut(&mut state, key)?.set_pinned(pinned);
        Ok(Self::write_status(changed))
    }

    async fn mark_content_dirty(
        &self,
        key: &ContentCacheKey,
        snapshot: LocalContentSnapshot,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let transition = Self::entry_mut(&mut state, key)?
            .mark_dirty(snapshot)
            .map_err(Self::transition_error)?;
        Ok(Self::dirty_transition_status(transition))
    }

    async fn clear_content_dirty(
        &self,
        key: &ContentCacheKey,
        generation: LocalContentGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let transition = Self::entry_mut(&mut state, key)?
            .clear_dirty(generation)
            .map_err(Self::transition_error)?;
        Ok(Self::dirty_transition_status(transition))
    }

    async fn acquire_content_lease(
        &self,
        key: &ContentCacheKey,
        lease_id: ContentLeaseId,
        kind: ContentLeaseKind,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let changed = Self::entry_mut(&mut state, key)?
            .acquire_lease(lease_id, kind)
            .map_err(Self::transition_error)?;
        Ok(Self::write_status(changed))
    }

    async fn release_content_lease(
        &self,
        key: &ContentCacheKey,
        lease_id: &ContentLeaseId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let changed = Self::entry_mut(&mut state, key)?
            .release_lease(lease_id)
            .map_err(Self::transition_error)?;
        Ok(Self::write_status(changed))
    }

    async fn begin_content_eviction(
        &self,
        intent: ContentEvictionIntent,
    ) -> StoreResult<ContentEvictionBegin> {
        let operation_id = intent.operation_id().clone();
        let cache_key = intent.cache_key().clone();
        let target = intent.target();
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");

        if let Some(existing) = state.evictions.get(&operation_id) {
            return if existing.intent() == &intent {
                Ok(ContentEvictionBegin::AlreadyPersisted {
                    plan: existing.plan(),
                })
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "content eviction operation id already identifies another intent",
                ))
            };
        }

        let decision = Self::entry_mut(&mut state, &cache_key)?
            .reserve_eviction(target)
            .map_err(Self::transition_error)?;
        let plan = match decision {
            ContentEvictionDecision::Blocked { blockers } => {
                return Ok(ContentEvictionBegin::Blocked { blockers });
            }
            ContentEvictionDecision::Reserved { plan } => plan,
        };
        state
            .evictions
            .insert(operation_id, ContentEvictionRecord::persist(intent, plan));
        drop(state);
        self.failures
            .trip(FailurePoint::AfterContentEvictionIntentPersistBeforeReturn)?;
        Ok(ContentEvictionBegin::Persisted { plan })
    }

    async fn load_content_eviction(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<Option<ContentEvictionRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        Ok(state.evictions.get(operation_id).cloned())
    }

    async fn recoverable_content_evictions(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Vec<ContentEvictionRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let mut records = state
            .evictions
            .values()
            .filter(|record| {
                record.intent().cache_key().item_key().scope() == scope
                    && record.state() != ContentEvictionState::Completed
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.intent()
                .operation_id()
                .as_str()
                .cmp(right.intent().operation_id().as_str())
        });
        Ok(records)
    }

    async fn record_content_eviction_effect(
        &self,
        operation_id: &ContentEvictionOperationId,
        effect: ContentEvictionPhysicalEffect,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.evictions.get_mut(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content eviction operation was not found",
            )
        })?;
        let transition = record
            .record_physical_effect(effect)
            .map_err(Self::transition_error)?;
        let status = Self::transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentEvictionEffectPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn reconcile_content_eviction_metadata(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.evictions.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content eviction operation was not found",
            )
        })?;
        if record.state() == ContentEvictionState::MetadataReconciled
            || record.state() == ContentEvictionState::Completed
        {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }

        let cache_key = record.intent().cache_key().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content storage entry for eviction was not found",
            )
        })?;
        next_entry
            .reconcile_eviction(next_record.plan())
            .map_err(Self::transition_error)?;
        let transition = next_record
            .mark_metadata_reconciled()
            .map_err(Self::transition_error)?;
        state.entries.insert(cache_key, next_entry);
        state.evictions.insert(operation_id.clone(), next_record);
        let status = Self::transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentEvictionMetadataPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn complete_content_eviction(
        &self,
        operation_id: &ContentEvictionOperationId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory content store mutex should not be poisoned");
        let record = state.evictions.get(operation_id).ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content eviction operation was not found",
            )
        })?;
        if record.state() == ContentEvictionState::Completed {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }

        let cache_key = record.intent().cache_key().clone();
        let mut next_record = record.clone();
        let mut next_entry = state.entries.get(&cache_key).cloned().ok_or_else(|| {
            Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "content storage entry for eviction was not found",
            )
        })?;
        let transition = next_record.complete().map_err(Self::transition_error)?;
        next_entry
            .complete_eviction()
            .map_err(Self::transition_error)?;
        state.entries.insert(cache_key, next_entry);
        state.evictions.insert(operation_id.clone(), next_record);
        let status = Self::transition_status(transition);
        drop(state);
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterContentEvictionCompletionPersistBeforeReturn)?;
        }
        Ok(status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticPhysicalContent {
    provider_cache_present: bool,
    platform_materialized: bool,
    platform_invalidation_count: usize,
}

impl SyntheticPhysicalContent {
    pub const fn new(provider_cache_present: bool, platform_materialized: bool) -> Self {
        Self {
            provider_cache_present,
            platform_materialized,
            platform_invalidation_count: 0,
        }
    }

    pub const fn provider_cache_present(&self) -> bool {
        self.provider_cache_present
    }

    pub const fn platform_materialized(&self) -> bool {
        self.platform_materialized
    }

    pub const fn platform_invalidation_count(&self) -> usize {
        self.platform_invalidation_count
    }

    pub fn remove_provider_cache(&mut self) {
        self.provider_cache_present = false;
    }

    pub fn evict_platform_materialization(&mut self) {
        self.platform_materialized = false;
    }

    pub fn invalidate_platform_content(&mut self) {
        self.platform_invalidation_count = self.platform_invalidation_count.saturating_add(1);
    }
}
