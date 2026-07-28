use std::{collections::HashMap, sync::Mutex};

use aster_forge_cloud_files_core::{
    ChangeBatchId, ChangeCheckpointStore, ChangeCursor, ChangeCursorCheckpoint,
    CloudFilesCoreError, CloudFilesStoreError, CloudFilesStoreErrorKind, CloudScope,
    IdempotencyKey, MutationIntent, MutationJournalStore, MutationRecord, MutationRecordTransition,
    MutationRemoteOutcome, MutationState, OperationId, PersistedChangeBatch,
    PersistedChangeBatchState, SessionGeneration, SessionState, StoreResult, StoreWriteStatus,
};
use async_trait::async_trait;

use super::failure_injection::{FailureInjector, FailurePoint};

#[derive(Debug, Clone)]
struct CursorState {
    active_cursor: Option<ChangeCursor>,
    pending_batch: Option<PersistedChangeBatch>,
    committed_batches: HashMap<ChangeBatchId, PersistedChangeBatch>,
}

impl CursorState {
    fn checkpoint(&self, scope: CloudScope) -> ChangeCursorCheckpoint {
        ChangeCursorCheckpoint::new(
            scope,
            self.active_cursor.clone(),
            self.pending_batch.clone(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct SessionRecord {
    generation: SessionGeneration,
    state: SessionState,
}

#[derive(Debug, Default)]
struct StoreState {
    cursors: HashMap<CloudScope, CursorState>,
    sessions: HashMap<CloudScope, SessionRecord>,
    mutations: HashMap<OperationId, MutationRecord>,
    idempotency_keys: HashMap<IdempotencyKey, OperationId>,
}

#[derive(Debug, Default)]
pub struct MemoryCloudFilesStore {
    state: Mutex<StoreState>,
    failures: FailureInjector,
    stall_next_remote_apply: Mutex<bool>,
}

impl MemoryCloudFilesStore {
    pub fn failures(&self) -> &FailureInjector {
        &self.failures
    }

    pub fn stall_next_remote_apply(&self) {
        *self
            .stall_next_remote_apply
            .lock()
            .expect("mutation stall mutex should not be poisoned") = true;
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

    fn active_generation(
        state: &StoreState,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> StoreResult<bool> {
        let Some(active) = state.sessions.get(scope) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "no active session for mutation scope",
            ));
        };
        if active.generation != generation || active.state == SessionState::Closed {
            return Ok(false);
        }
        Ok(true)
    }

    fn mutation_scope(state: &StoreState, operation_id: &OperationId) -> StoreResult<CloudScope> {
        let Some(record) = state.mutations.get(operation_id) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "mutation operation was not found",
            ));
        };
        Ok(record.intent().desired().scope().clone())
    }

    fn with_fenced_mutation(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
        transition: impl FnOnce(
            &mut MutationRecord,
        ) -> Result<MutationRecordTransition, CloudFilesCoreError>,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let scope = Self::mutation_scope(&state, operation_id)?;
        if !Self::active_generation(&state, &scope, generation)? {
            return Ok(StoreWriteStatus::Fenced);
        }
        let record = state
            .mutations
            .get_mut(operation_id)
            .expect("mutation existence was checked above");
        transition(record)
            .map(|status| match status {
                MutationRecordTransition::Applied => StoreWriteStatus::Applied,
                MutationRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
            })
            .map_err(Self::transition_error)
    }
}

#[async_trait]
impl ChangeCheckpointStore for MemoryCloudFilesStore {
    async fn load_change_checkpoint(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<ChangeCursorCheckpoint> {
        let state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        Ok(state.cursors.get(scope).map_or_else(
            || ChangeCursorCheckpoint::initial(scope.clone()),
            |cursor| cursor.checkpoint(scope.clone()),
        ))
    }

    async fn record_change_batch(
        &self,
        batch: PersistedChangeBatch,
    ) -> StoreResult<StoreWriteStatus> {
        let scope = batch.scope().clone();
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let cursor = state.cursors.entry(scope).or_insert_with(|| CursorState {
            active_cursor: None,
            pending_batch: None,
            committed_batches: HashMap::new(),
        });

        if let Some(committed) = cursor.committed_batches.get(batch.id()) {
            return if committed == &batch {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "change batch id already identifies different committed content",
                ))
            };
        }
        if let Some(pending) = &cursor.pending_batch {
            if pending == &batch {
                return Ok(StoreWriteStatus::AlreadyApplied);
            }
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "another change batch is already pending",
            ));
        }
        if cursor.active_cursor.as_ref() != batch.base_cursor() {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "change batch base cursor does not match active cursor",
            ));
        }
        cursor.pending_batch = Some(batch);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterChangeBatchPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }

    async fn mark_change_effects_replayable(
        &self,
        scope: &CloudScope,
        batch_id: &ChangeBatchId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let Some(cursor) = state.cursors.get_mut(scope) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "change checkpoint was not found",
            ));
        };
        if cursor.committed_batches.contains_key(batch_id) {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let Some(pending) = cursor.pending_batch.as_mut() else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "pending change batch was not found",
            ));
        };
        if pending.id() != batch_id {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "pending change batch identity does not match",
            ));
        }
        let changed = pending.mark_effects_replayable();
        drop(state);
        self.failures
            .trip(FailurePoint::AfterChangeEffectsReplayablePersistBeforeReturn)?;
        Ok(if changed {
            StoreWriteStatus::Applied
        } else {
            StoreWriteStatus::AlreadyApplied
        })
    }

    async fn commit_change_cursor(
        &self,
        scope: &CloudScope,
        batch_id: &ChangeBatchId,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let Some(cursor) = state.cursors.get_mut(scope) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "change checkpoint was not found",
            ));
        };
        if cursor.committed_batches.contains_key(batch_id) {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        let Some(pending) = cursor.pending_batch.as_ref() else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "pending change batch was not found",
            ));
        };
        if pending.id() != batch_id {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "pending change batch identity does not match",
            ));
        }
        if pending.state() != PersistedChangeBatchState::EffectsReplayable {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "change cursor cannot advance before effects are replayable",
            ));
        }
        let committed_batch = pending.clone();
        let next_cursor = committed_batch.batch().next_cursor().clone();
        cursor.active_cursor = Some(next_cursor);
        cursor.pending_batch = None;
        cursor
            .committed_batches
            .insert(batch_id.clone(), committed_batch);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterCursorCommitPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }
}

#[async_trait]
impl MutationJournalStore for MemoryCloudFilesStore {
    async fn activate_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        match state.sessions.get(scope).copied() {
            None => {
                state.sessions.insert(
                    scope.clone(),
                    SessionRecord {
                        generation,
                        state: SessionState::Accepting,
                    },
                );
                Ok(StoreWriteStatus::Applied)
            }
            Some(active) if generation < active.generation => Ok(StoreWriteStatus::Fenced),
            Some(active) if generation == active.generation => {
                if active.state == SessionState::Accepting {
                    Ok(StoreWriteStatus::AlreadyApplied)
                } else {
                    Err(Self::store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "the same generation cannot reopen a closing or closed session",
                    ))
                }
            }
            Some(_) => {
                state.sessions.insert(
                    scope.clone(),
                    SessionRecord {
                        generation,
                        state: SessionState::Accepting,
                    },
                );
                Ok(StoreWriteStatus::Applied)
            }
        }
    }

    async fn load_session(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Option<(SessionGeneration, SessionState)>> {
        let state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        Ok(state
            .sessions
            .get(scope)
            .map(|session| (session.generation, session.state)))
    }

    async fn transition_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
        next: SessionState,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let Some(session) = state.sessions.get_mut(scope) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "session was not found",
            ));
        };
        if generation < session.generation {
            return Ok(StoreWriteStatus::Fenced);
        }
        if generation > session.generation {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "requested generation is not active",
            ));
        }
        if session.state == next {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        if !session.state.can_transition_to(next) {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "invalid session lifecycle transition",
            ));
        }
        session.state = next;
        Ok(StoreWriteStatus::Applied)
    }

    async fn persist_mutation_intent(
        &self,
        intent: MutationIntent,
    ) -> StoreResult<StoreWriteStatus> {
        self.failures
            .trip(FailurePoint::BeforeMutationIntentPersist)?;
        let record = MutationRecord::persist(intent).map_err(Self::transition_error)?;
        let operation_id = record.intent().operation_id().clone();
        let idempotency_key = record.intent().idempotency_key().clone();
        let scope = record.intent().desired().scope().clone();
        let generation = record.intent().session_generation();

        let mut state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let Some(session) = state.sessions.get(&scope) else {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::NotFound,
                "no active session for mutation intent",
            ));
        };
        if session.generation != generation || session.state != SessionState::Accepting {
            return Ok(StoreWriteStatus::Fenced);
        }
        if let Some(existing) = state.mutations.get(&operation_id) {
            return if existing == &record {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(Self::store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "operation id already identifies a different mutation",
                ))
            };
        }
        if let Some(existing_operation) = state.idempotency_keys.get(&idempotency_key)
            && existing_operation != &operation_id
        {
            return Err(Self::store_error(
                CloudFilesStoreErrorKind::Conflict,
                "idempotency key already identifies another operation",
            ));
        }
        state
            .idempotency_keys
            .insert(idempotency_key, operation_id.clone());
        state.mutations.insert(operation_id, record);
        drop(state);
        self.failures
            .trip(FailurePoint::AfterMutationIntentPersistBeforeReturn)?;
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_mutation(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<MutationRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        Ok(state.mutations.get(operation_id).cloned())
    }

    async fn recoverable_mutations(&self, scope: &CloudScope) -> StoreResult<Vec<MutationRecord>> {
        let state = self
            .state
            .lock()
            .expect("memory store mutex should not be poisoned");
        let mut records = state
            .mutations
            .values()
            .filter(|record| {
                record.intent().desired().scope() == scope
                    && record.state() != MutationState::Completed
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

    async fn begin_remote_apply(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        if std::mem::take(
            &mut *self
                .stall_next_remote_apply
                .lock()
                .expect("mutation stall mutex should not be poisoned"),
        ) {
            return Ok(StoreWriteStatus::Applied);
        }
        let status = self.with_fenced_mutation(
            operation_id,
            generation,
            MutationRecord::begin_remote_apply,
        )?;
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterRemoteApplyPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn record_remote_outcome(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
        outcome: MutationRemoteOutcome,
    ) -> StoreResult<StoreWriteStatus> {
        let status = self.with_fenced_mutation(operation_id, generation, |record| {
            record.record_remote_outcome(outcome)
        })?;
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterRemoteOutcomePersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn mark_platform_reconciled(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let status = self.with_fenced_mutation(
            operation_id,
            generation,
            MutationRecord::mark_platform_reconciled,
        )?;
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterPlatformReconciledPersistBeforeReturn)?;
        }
        Ok(status)
    }

    async fn complete_mutation(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let status =
            self.with_fenced_mutation(operation_id, generation, MutationRecord::complete)?;
        if status == StoreWriteStatus::Applied {
            self.failures
                .trip(FailurePoint::AfterMutationCompletionPersistBeforeReturn)?;
        }
        Ok(status)
    }
}
