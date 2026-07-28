mod support;

use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use aster_forge_cloud_files_core::{
    BackendResult, CloudBackendError, CloudBackendErrorKind, CloudContentMetadata,
    CloudFilesCoreError, CloudItem, CloudItemId, CloudItemKey, CloudItemKind, CloudMutationBackend,
    CloudNamespaceId, CloudRootId, CloudScope, ContentRevision, DesiredMutation, IdempotencyKey,
    LocalContentGeneration, LocalContentReference, LocalContentSnapshot, MetadataRevision,
    MutationIntent, MutationJournalStore, MutationOrigin, MutationPreconditions,
    MutationRemoteOutcome, MutationRunError, MutationRunOutcome, MutationRunner, MutationState,
    OperationId, SessionGeneration,
};
use async_trait::async_trait;
use futures::channel::oneshot;

use support::{SyntheticBackend, failure_injection::FailurePoint, store::MemoryCloudFilesStore};

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn operation(value: &str) -> OperationId {
    OperationId::new(value).expect("operation fixture should be valid")
}

fn idempotency(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("idempotency fixture should be valid")
}

fn metadata_revision(value: &str) -> MetadataRevision {
    MetadataRevision::from_slice(value.as_bytes())
        .expect("metadata revision fixture should be valid")
}

fn content_metadata(value: &str, size: u64) -> CloudContentMetadata {
    CloudContentMetadata::new(
        ContentRevision::from_slice(value.as_bytes())
            .expect("content revision fixture should be valid"),
        None,
        size,
    )
}

fn create_intent(fixture: &SyntheticBackend, operation_value: &str) -> MutationIntent {
    MutationIntent::new(
        operation(operation_value),
        idempotency(&format!("{operation_value}-idempotency")),
        MutationOrigin::PlatformCommand,
        generation(1),
        DesiredMutation::create(
            fixture.scope().clone(),
            fixture.root_key().item_id().clone(),
            "created.txt",
            CloudItemKind::File,
        )
        .expect("create mutation fixture should be valid"),
        MutationPreconditions::default(),
    )
}

fn created_file(fixture: &SyntheticBackend, item_id: &str) -> CloudItem {
    CloudItem::file(
        CloudItemKey::new(
            fixture.scope().clone(),
            CloudItemId::new(item_id).expect("item id fixture should be valid"),
        ),
        fixture.root_key().item_id().clone(),
        "created.txt",
        metadata_revision("created-m1"),
        content_metadata("created-c1", 0),
    )
    .expect("created file fixture should be valid")
}

fn delete_intent(fixture: &SyntheticBackend, operation_value: &str) -> MutationIntent {
    MutationIntent::new(
        operation(operation_value),
        idempotency(&format!("{operation_value}-idempotency")),
        MutationOrigin::PlatformCommand,
        generation(1),
        DesiredMutation::delete(fixture.deleted_file().key().clone()),
        MutationPreconditions::default(),
    )
}

fn content_intent(fixture: &SyntheticBackend, operation_value: &str) -> MutationIntent {
    let key = fixture.moved_file().key().clone();
    MutationIntent::new(
        operation(operation_value),
        idempotency(&format!("{operation_value}-idempotency")),
        MutationOrigin::PlatformObserved,
        generation(1),
        DesiredMutation::modify_content(key.clone()),
        MutationPreconditions::default(),
    )
    .with_local_content(LocalContentSnapshot::new(
        key,
        LocalContentGeneration::new(1).expect("local generation fixture should be valid"),
        LocalContentReference::new(format!("{operation_value}-snapshot"))
            .expect("local snapshot reference should be valid"),
        17,
        None,
    ))
}

fn metadata_intent(fixture: &SyntheticBackend, operation_value: &str) -> MutationIntent {
    MutationIntent::new(
        operation(operation_value),
        idempotency(&format!("{operation_value}-idempotency")),
        MutationOrigin::PlatformPreflight,
        generation(1),
        DesiredMutation::modify_metadata(
            fixture.moved_file_before().key().clone(),
            fixture.moved_file().parent_id().cloned(),
            Some(fixture.moved_file().name().to_owned()),
        )
        .expect("metadata mutation fixture should be valid"),
        MutationPreconditions::default(),
    )
}

async fn activate(store: &MemoryCloudFilesStore, fixture: &SyntheticBackend, value: u64) {
    store
        .activate_session(fixture.scope(), generation(value))
        .await
        .expect("session activation should succeed");
}

#[derive(Debug, Clone)]
enum BackendStep {
    Outcome(Box<MutationRemoteOutcome>),
    Error(CloudBackendErrorKind),
}

impl BackendStep {
    fn outcome(outcome: MutationRemoteOutcome) -> Self {
        Self::Outcome(Box::new(outcome))
    }

    fn execute(self) -> BackendResult<MutationRemoteOutcome> {
        match self {
            Self::Outcome(outcome) => Ok(*outcome),
            Self::Error(kind) => Err(CloudBackendError::retryable(kind)),
        }
    }
}

#[derive(Debug, Default)]
struct ScriptedBackendState {
    apply: VecDeque<BackendStep>,
    reconcile: VecDeque<BackendStep>,
    apply_fallback: Option<BackendStep>,
    reconcile_fallback: Option<BackendStep>,
    apply_calls: usize,
    reconcile_calls: usize,
}

#[derive(Debug, Default)]
struct ScriptedMutationBackend {
    state: Mutex<ScriptedBackendState>,
}

impl ScriptedMutationBackend {
    fn new(
        apply: impl IntoIterator<Item = BackendStep>,
        reconcile: impl IntoIterator<Item = BackendStep>,
    ) -> Self {
        Self {
            state: Mutex::new(ScriptedBackendState {
                apply: apply.into_iter().collect(),
                reconcile: reconcile.into_iter().collect(),
                ..ScriptedBackendState::default()
            }),
        }
    }

    fn fixed_apply(outcome: MutationRemoteOutcome) -> Self {
        let backend = Self::default();
        backend
            .state
            .lock()
            .expect("scripted backend mutex should not be poisoned")
            .apply_fallback = Some(BackendStep::outcome(outcome));
        backend
    }

    fn calls(&self) -> (usize, usize) {
        let state = self
            .state
            .lock()
            .expect("scripted backend mutex should not be poisoned");
        (state.apply_calls, state.reconcile_calls)
    }

    fn next_step(
        queue: &mut VecDeque<BackendStep>,
        fallback: &Option<BackendStep>,
    ) -> BackendResult<MutationRemoteOutcome> {
        queue
            .pop_front()
            .or_else(|| fallback.clone())
            .unwrap_or(BackendStep::Error(CloudBackendErrorKind::Internal))
            .execute()
    }
}

#[async_trait]
impl CloudMutationBackend for ScriptedMutationBackend {
    async fn apply_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let mut state = self
            .state
            .lock()
            .expect("scripted backend mutex should not be poisoned");
        state.apply_calls += 1;
        let fallback = state.apply_fallback.clone();
        Self::next_step(&mut state.apply, &fallback)
    }

    async fn reconcile_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let mut state = self
            .state
            .lock()
            .expect("scripted backend mutex should not be poisoned");
        state.reconcile_calls += 1;
        let fallback = state.reconcile_fallback.clone();
        Self::next_step(&mut state.reconcile, &fallback)
    }
}

#[derive(Debug)]
struct ConcurrentMutationBackend {
    calls: AtomicUsize,
    release_first: Mutex<Option<oneshot::Sender<()>>>,
    item: CloudItem,
}

impl ConcurrentMutationBackend {
    fn new(item: CloudItem) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            release_first: Mutex::new(None),
            item,
        }
    }
}

#[async_trait]
impl CloudMutationBackend for ConcurrentMutationBackend {
    async fn apply_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let (release, wait) = oneshot::channel();
            *self
                .release_first
                .lock()
                .expect("concurrent backend mutex should not be poisoned") = Some(release);
            wait.await
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::Internal))?;
        } else {
            self.release_first
                .lock()
                .expect("concurrent backend mutex should not be poisoned")
                .take()
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::Internal))?
                .send(())
                .map_err(|()| CloudBackendError::new(CloudBackendErrorKind::Internal))?;
        }
        if call == 0 {
            Ok(MutationRemoteOutcome::Committed {
                item: Some(self.item.clone()),
            })
        } else {
            Ok(MutationRemoteOutcome::AlreadyCommitted {
                item: Some(self.item.clone()),
            })
        }
    }

    async fn reconcile_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        Err(CloudBackendError::new(CloudBackendErrorKind::Internal))
    }
}

#[derive(Debug)]
struct GenerationTakeoverBackend<'a> {
    store: &'a MemoryCloudFilesStore,
    scope: CloudScope,
    calls: AtomicUsize,
}

#[async_trait]
impl CloudMutationBackend for GenerationTakeoverBackend<'_> {
    async fn apply_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.store
                .activate_session(&self.scope, generation(2))
                .await
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::Internal))?;
            Ok(MutationRemoteOutcome::Committed { item: None })
        } else {
            Ok(MutationRemoteOutcome::AlreadyCommitted { item: None })
        }
    }

    async fn reconcile_mutation(
        &self,
        _intent: &MutationIntent,
    ) -> BackendResult<MutationRemoteOutcome> {
        Err(CloudBackendError::new(CloudBackendErrorKind::Internal))
    }
}

#[tokio::test]
async fn submit_drives_a_durable_create_through_product_reconciliation() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = create_intent(&fixture, "create-complete");
    let operation_id = intent.operation_id().clone();
    let item = created_file(&fixture, "created-item");
    let backend = ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed {
        item: Some(item.clone()),
    });

    assert_eq!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await
            .expect("create runner should complete"),
        MutationRunOutcome::Completed
    );
    let record = store
        .load_mutation(&operation_id)
        .await
        .expect("mutation load should succeed")
        .expect("mutation should be durable");
    assert_eq!(record.state(), MutationState::Completed);
    assert_eq!(
        record.remote_outcome(),
        Some(&MutationRemoteOutcome::Committed { item: Some(item) })
    );
    assert_eq!(backend.calls(), (1, 0));
}

#[tokio::test]
async fn resume_advances_an_existing_intent_without_reinserting_it() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "resume-existing");
    let operation_id = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent should persist");
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });

    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("existing mutation should complete"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls(), (1, 0));
}

#[tokio::test]
async fn known_remote_outcomes_all_reach_completion() {
    let fixture = SyntheticBackend::full();
    for (suffix, outcome) in [
        (
            "committed",
            MutationRemoteOutcome::Committed {
                item: Some(created_file(&fixture, "known-committed")),
            },
        ),
        (
            "already",
            MutationRemoteOutcome::AlreadyCommitted {
                item: Some(created_file(&fixture, "known-already")),
            },
        ),
        (
            "precondition",
            MutationRemoteOutcome::PreconditionFailed {
                metadata_revision: Some(metadata_revision("current-metadata")),
                content_revision: None,
            },
        ),
    ] {
        let store = MemoryCloudFilesStore::default();
        activate(&store, &fixture, 1).await;
        let backend = ScriptedMutationBackend::fixed_apply(outcome.clone());
        assert_eq!(
            MutationRunner
                .submit(
                    create_intent(&fixture, &format!("known-{suffix}")),
                    generation(1),
                    &store,
                    &backend,
                )
                .await
                .expect("known outcome should complete"),
            MutationRunOutcome::Completed
        );
    }
}

#[tokio::test]
async fn unknown_outcome_waits_for_status_reconciliation_without_busy_looping() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = create_intent(&fixture, "unknown-create");
    let operation_id = intent.operation_id().clone();
    let backend = ScriptedMutationBackend::new(
        [BackendStep::outcome(
            MutationRemoteOutcome::RemoteOutcomeUnknown,
        )],
        [
            BackendStep::outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
            BackendStep::outcome(MutationRemoteOutcome::AlreadyCommitted {
                item: Some(created_file(&fixture, "unknown-created-item")),
            }),
        ],
    );

    assert_eq!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await
            .expect("unknown application should remain pending"),
        MutationRunOutcome::RemoteOutcomePending
    );
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("still-unknown reconciliation should remain pending"),
        MutationRunOutcome::RemoteOutcomePending
    );
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("known reconciliation should complete"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls(), (1, 2));
}

#[tokio::test]
async fn backend_failures_leave_the_last_durable_retry_point() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "apply-failure");
    let operation_id = intent.operation_id().clone();
    let backend = ScriptedMutationBackend::new(
        [BackendStep::Error(
            CloudBackendErrorKind::TemporarilyUnavailable,
        )],
        [],
    );
    assert!(matches!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await,
        Err(MutationRunError::Backend(_))
    ));
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::RemoteApplying
    );

    let reconcile_store = MemoryCloudFilesStore::default();
    activate(&reconcile_store, &fixture, 1).await;
    let reconcile_intent = delete_intent(&fixture, "reconcile-failure");
    let reconcile_operation = reconcile_intent.operation_id().clone();
    let reconcile_backend = ScriptedMutationBackend::new(
        [BackendStep::outcome(
            MutationRemoteOutcome::RemoteOutcomeUnknown,
        )],
        [BackendStep::Error(
            CloudBackendErrorKind::TemporarilyUnavailable,
        )],
    );
    assert_eq!(
        MutationRunner
            .submit(
                reconcile_intent,
                generation(1),
                &reconcile_store,
                &reconcile_backend,
            )
            .await
            .expect("unknown apply should become pending"),
        MutationRunOutcome::RemoteOutcomePending
    );
    assert!(matches!(
        MutationRunner
            .resume(
                &reconcile_operation,
                generation(1),
                &reconcile_store,
                &reconcile_backend,
            )
            .await,
        Err(MutationRunError::Backend(_))
    ));
    assert_eq!(
        reconcile_store
            .load_mutation(&reconcile_operation)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::RemoteOutcomeUnknown
    );
}

#[tokio::test]
async fn retry_after_a_lost_remote_return_uses_the_same_durable_intent() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "lost-remote-return");
    let operation_id = intent.operation_id().clone();
    let backend = ScriptedMutationBackend::new(
        [
            BackendStep::Error(CloudBackendErrorKind::TemporarilyUnavailable),
            BackendStep::outcome(MutationRemoteOutcome::AlreadyCommitted { item: None }),
        ],
        [],
    );

    assert!(matches!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await,
        Err(MutationRunError::Backend(_))
    ));
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("idempotent replay should complete"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls(), (2, 0));
}

#[tokio::test]
async fn every_store_transition_recovers_after_a_lost_persist_return() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "lost-store-returns");
    let operation_id = intent.operation_id().clone();
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });

    store
        .failures()
        .arm(FailurePoint::AfterMutationIntentPersistBeforeReturn);
    assert!(matches!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await,
        Err(MutationRunError::Store(_))
    ));
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("intent should be durable")
            .state(),
        MutationState::IntentPersisted
    );

    for (failure, expected_state) in [
        (
            FailurePoint::AfterRemoteApplyPersistBeforeReturn,
            MutationState::RemoteApplying,
        ),
        (
            FailurePoint::AfterRemoteOutcomePersistBeforeReturn,
            MutationState::RemoteOutcomeKnown,
        ),
        (
            FailurePoint::AfterPlatformReconciledPersistBeforeReturn,
            MutationState::PlatformReconciled,
        ),
        (
            FailurePoint::AfterMutationCompletionPersistBeforeReturn,
            MutationState::Completed,
        ),
    ] {
        store.failures().arm(failure);
        assert!(matches!(
            MutationRunner
                .resume(&operation_id, generation(1), &store, &backend)
                .await,
            Err(MutationRunError::Store(_))
        ));
        assert_eq!(
            store
                .load_mutation(&operation_id)
                .await
                .expect("mutation load should succeed")
                .expect("mutation should remain durable")
                .state(),
            expected_state
        );
    }

    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("completed replay should succeed"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls(), (1, 0));
}

#[tokio::test]
async fn stalled_store_transition_is_a_contract_failure() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    store.stall_next_remote_apply();
    let intent = delete_intent(&fixture, "stalled-transition");
    let operation_id = intent.operation_id().clone();
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });

    assert!(matches!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await,
        Err(MutationRunError::Contract(
            CloudFilesCoreError::InvalidMutationTransition { .. }
        ))
    ));
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::IntentPersisted
    );
    assert_eq!(backend.calls(), (0, 0));
}

#[tokio::test]
async fn newer_generation_fences_the_old_runner_and_resumes_the_same_intent() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "generation-takeover");
    let operation_id = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("old intent should persist");
    activate(&store, &fixture, 2).await;
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });

    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("old runner should be fenced"),
        MutationRunOutcome::Fenced
    );
    assert_eq!(backend.calls(), (0, 0));
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(2), &store, &backend)
            .await
            .expect("new runner should resume the old intent"),
        MutationRunOutcome::Completed
    );
}

#[tokio::test]
async fn generation_takeover_after_remote_apply_fences_the_late_outcome_write() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "late-outcome-fence");
    let operation_id = intent.operation_id().clone();
    let backend = GenerationTakeoverBackend {
        store: &store,
        scope: fixture.scope().clone(),
        calls: AtomicUsize::new(0),
    };

    assert_eq!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await
            .expect("old executor outcome should be classified"),
        MutationRunOutcome::Fenced
    );
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::RemoteApplying
    );
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(2), &store, &backend)
            .await
            .expect("new executor should idempotently resolve the remote effect"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn two_concurrent_runners_converge_on_one_completed_record() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = create_intent(&fixture, "concurrent-runners");
    let operation_id = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent should persist");
    let backend = ConcurrentMutationBackend::new(created_file(&fixture, "concurrent-created"));

    let (first, second) = tokio::join!(
        MutationRunner.resume(&operation_id, generation(1), &store, &backend),
        MutationRunner.resume(&operation_id, generation(1), &store, &backend),
    );
    assert_eq!(
        first.expect("first runner should converge"),
        MutationRunOutcome::Completed
    );
    assert_eq!(
        second.expect("second runner should converge"),
        MutationRunOutcome::Completed
    );
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::Completed
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn submit_rejects_an_executor_generation_that_does_not_match_the_intent() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let backend = ScriptedMutationBackend::default();
    assert!(matches!(
        MutationRunner
            .submit(
                delete_intent(&fixture, "generation-mismatch"),
                generation(2),
                &store,
                &backend,
            )
            .await,
        Err(MutationRunError::Contract(
            CloudFilesCoreError::InvalidMutationIntent { .. }
        ))
    ));
    assert_eq!(backend.calls(), (0, 0));
}

#[tokio::test]
async fn missing_record_is_reported_without_touching_the_backend() {
    let store = MemoryCloudFilesStore::default();
    let backend = ScriptedMutationBackend::default();
    assert_eq!(
        MutationRunner
            .resume(
                &operation("missing-mutation"),
                generation(1),
                &store,
                &backend
            )
            .await,
        Err(MutationRunError::RecordNotFound)
    );
    assert_eq!(backend.calls(), (0, 0));
}

async fn assert_contract_outcome(
    fixture: &SyntheticBackend,
    intent: MutationIntent,
    outcome: MutationRemoteOutcome,
) {
    let operation_id = intent.operation_id().as_str().to_owned();
    let store = MemoryCloudFilesStore::default();
    activate(&store, fixture, 1).await;
    let backend = ScriptedMutationBackend::fixed_apply(outcome);
    let result = MutationRunner
        .submit(intent, generation(1), &store, &backend)
        .await;
    assert!(
        matches!(
            result,
            Err(MutationRunError::Contract(
                CloudFilesCoreError::InvalidMutationTransition { .. }
            ))
        ),
        "{operation_id} returned {result:?}"
    );
}

#[tokio::test]
async fn create_outcome_must_match_scope_parent_name_and_kind() {
    let fixture = SyntheticBackend::full();
    assert_contract_outcome(
        &fixture,
        create_intent(&fixture, "create-missing-item"),
        MutationRemoteOutcome::Committed { item: None },
    )
    .await;

    let foreign_scope = CloudScope::new(
        CloudNamespaceId::new("foreign-provider").expect("namespace should be valid"),
        CloudRootId::new("foreign-root").expect("root should be valid"),
    );
    let wrong_scope = CloudItem::file(
        CloudItemKey::new(
            foreign_scope,
            CloudItemId::new("foreign-create").expect("item id should be valid"),
        ),
        fixture.root_key().item_id().clone(),
        "created.txt",
        metadata_revision("wrong-scope-m1"),
        content_metadata("wrong-scope-c1", 0),
    )
    .expect("foreign item fixture should be valid");
    assert_contract_outcome(
        &fixture,
        create_intent(&fixture, "create-wrong-scope"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_scope),
        },
    )
    .await;

    let wrong_parent = CloudItem::file(
        CloudItemKey::new(
            fixture.scope().clone(),
            CloudItemId::new("wrong-parent-create").expect("item id should be valid"),
        ),
        fixture
            .moved_file()
            .parent_id()
            .expect("moved file should have a parent")
            .clone(),
        "created.txt",
        metadata_revision("wrong-parent-m1"),
        content_metadata("wrong-parent-c1", 0),
    )
    .expect("wrong parent fixture should be valid");
    assert_contract_outcome(
        &fixture,
        create_intent(&fixture, "create-wrong-parent"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_parent),
        },
    )
    .await;

    let wrong_name = CloudItem::file(
        CloudItemKey::new(
            fixture.scope().clone(),
            CloudItemId::new("wrong-name-create").expect("item id should be valid"),
        ),
        fixture.root_key().item_id().clone(),
        "other.txt",
        metadata_revision("wrong-name-m1"),
        content_metadata("wrong-name-c1", 0),
    )
    .expect("wrong name fixture should be valid");
    assert_contract_outcome(
        &fixture,
        create_intent(&fixture, "create-wrong-name"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_name),
        },
    )
    .await;

    let wrong_kind = CloudItem::directory(
        CloudItemKey::new(
            fixture.scope().clone(),
            CloudItemId::new("wrong-kind-create").expect("item id should be valid"),
        ),
        Some(fixture.root_key().item_id().clone()),
        "created.txt",
        metadata_revision("wrong-kind-m1"),
    )
    .expect("wrong kind fixture should be valid");
    assert_contract_outcome(
        &fixture,
        create_intent(&fixture, "create-wrong-kind"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_kind),
        },
    )
    .await;
}

#[tokio::test]
async fn existing_item_outcomes_must_match_target_and_requested_shape() {
    let fixture = SyntheticBackend::full();
    assert_contract_outcome(
        &fixture,
        content_intent(&fixture, "content-missing-item"),
        MutationRemoteOutcome::Committed { item: None },
    )
    .await;
    assert_contract_outcome(
        &fixture,
        content_intent(&fixture, "content-wrong-item"),
        MutationRemoteOutcome::Committed {
            item: Some(fixture.deleted_file().clone()),
        },
    )
    .await;
    let directory = CloudItem::directory(
        fixture.moved_file().key().clone(),
        fixture.moved_file().parent_id().cloned(),
        fixture.moved_file().name(),
        metadata_revision("content-directory-m1"),
    )
    .expect("directory outcome fixture should be valid");
    assert_contract_outcome(
        &fixture,
        content_intent(&fixture, "content-directory"),
        MutationRemoteOutcome::Committed {
            item: Some(directory),
        },
    )
    .await;

    assert_contract_outcome(
        &fixture,
        metadata_intent(&fixture, "metadata-wrong-item"),
        MutationRemoteOutcome::Committed {
            item: Some(fixture.deleted_file().clone()),
        },
    )
    .await;
    let wrong_metadata_parent = fixture
        .moved_file_before()
        .clone()
        .moved(
            fixture.root_key().item_id().clone(),
            fixture.moved_file().name(),
            metadata_revision("metadata-wrong-parent"),
        )
        .expect("wrong metadata parent fixture should be valid");
    assert_contract_outcome(
        &fixture,
        metadata_intent(&fixture, "metadata-wrong-parent"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_metadata_parent),
        },
    )
    .await;
    let wrong_metadata_name = fixture
        .moved_file_before()
        .clone()
        .moved(
            fixture
                .moved_file()
                .parent_id()
                .expect("moved file should have a parent")
                .clone(),
            "wrong-metadata-name.txt",
            metadata_revision("metadata-wrong-name"),
        )
        .expect("wrong metadata name fixture should be valid");
    assert_contract_outcome(
        &fixture,
        metadata_intent(&fixture, "metadata-wrong-name"),
        MutationRemoteOutcome::Committed {
            item: Some(wrong_metadata_name),
        },
    )
    .await;
}

#[tokio::test]
async fn delete_accepts_no_item_but_rejects_another_item() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });
    assert_eq!(
        MutationRunner
            .submit(
                delete_intent(&fixture, "delete-without-item"),
                generation(1),
                &store,
                &backend,
            )
            .await
            .expect("delete without current metadata should complete"),
        MutationRunOutcome::Completed
    );

    let matching_store = MemoryCloudFilesStore::default();
    activate(&matching_store, &fixture, 1).await;
    let matching_backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::AlreadyCommitted {
            item: Some(fixture.deleted_file().clone()),
        });
    assert_eq!(
        MutationRunner
            .submit(
                delete_intent(&fixture, "delete-matching-item"),
                generation(1),
                &matching_store,
                &matching_backend,
            )
            .await
            .expect("delete with matching current metadata should complete"),
        MutationRunOutcome::Completed
    );

    assert_contract_outcome(
        &fixture,
        delete_intent(&fixture, "delete-wrong-item"),
        MutationRemoteOutcome::AlreadyCommitted {
            item: Some(fixture.moved_file().clone()),
        },
    )
    .await;
}

#[tokio::test]
async fn reconciliation_outcome_is_validated_before_platform_state_changes() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = create_intent(&fixture, "invalid-reconciliation");
    let operation_id = intent.operation_id().clone();
    let backend = ScriptedMutationBackend::new(
        [BackendStep::outcome(
            MutationRemoteOutcome::RemoteOutcomeUnknown,
        )],
        [BackendStep::outcome(
            MutationRemoteOutcome::AlreadyCommitted { item: None },
        )],
    );
    assert_eq!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await
            .expect("initial unknown should remain pending"),
        MutationRunOutcome::RemoteOutcomePending
    );
    assert!(matches!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await,
        Err(MutationRunError::Contract(_))
    ));
    assert_eq!(
        store
            .load_mutation(&operation_id)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::RemoteOutcomeUnknown
    );
}

#[tokio::test]
async fn completed_replay_does_not_repeat_the_remote_effect() {
    let fixture = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    activate(&store, &fixture, 1).await;
    let intent = delete_intent(&fixture, "completed-replay");
    let operation_id = intent.operation_id().clone();
    let backend =
        ScriptedMutationBackend::fixed_apply(MutationRemoteOutcome::Committed { item: None });
    assert_eq!(
        MutationRunner
            .submit(intent, generation(1), &store, &backend)
            .await
            .expect("initial mutation should complete"),
        MutationRunOutcome::Completed
    );
    assert_eq!(
        MutationRunner
            .resume(&operation_id, generation(1), &store, &backend)
            .await
            .expect("completed replay should remain complete"),
        MutationRunOutcome::Completed
    );
    assert_eq!(backend.calls(), (1, 0));
}
