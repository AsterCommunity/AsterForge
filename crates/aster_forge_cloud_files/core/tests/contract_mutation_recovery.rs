mod support;

use std::collections::{HashMap, HashSet};

use aster_forge_cloud_files_core::{
    CloudFilesStoreErrorKind, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
    DesiredMutation, IdempotencyKey, LocalContentGeneration, LocalContentReference,
    LocalContentSnapshot, MutationIntent, MutationJournalStore, MutationOrigin,
    MutationPreconditions, MutationRemoteOutcome, MutationState, OperationId, SessionGeneration,
    SessionState, StoreWriteStatus,
};

use support::{SyntheticBackend, failure_injection::FailurePoint, store::MemoryCloudFilesStore};

#[derive(Default)]
struct SyntheticMutationRemote {
    committed: HashSet<IdempotencyKey>,
    attempts: HashMap<IdempotencyKey, usize>,
}

impl SyntheticMutationRemote {
    fn apply(&mut self, key: &IdempotencyKey) -> MutationRemoteOutcome {
        *self.attempts.entry(key.clone()).or_default() += 1;
        if self.committed.insert(key.clone()) {
            MutationRemoteOutcome::Committed { item: None }
        } else {
            MutationRemoteOutcome::AlreadyCommitted { item: None }
        }
    }

    fn reconcile(&self, key: &IdempotencyKey) -> Option<MutationRemoteOutcome> {
        self.committed
            .contains(key)
            .then_some(MutationRemoteOutcome::AlreadyCommitted { item: None })
    }

    fn attempts(&self, key: &IdempotencyKey) -> usize {
        self.attempts.get(key).copied().unwrap_or_default()
    }
}

fn operation_id(value: &str) -> OperationId {
    OperationId::new(value).expect("operation id fixture should be valid")
}

fn idempotency_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("idempotency key fixture should be valid")
}

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn delete_intent(
    backend: &SyntheticBackend,
    operation: &str,
    idempotency: &str,
    generation: SessionGeneration,
    origin: MutationOrigin,
) -> MutationIntent {
    MutationIntent::new(
        operation_id(operation),
        idempotency_key(idempotency),
        origin,
        generation,
        DesiredMutation::delete(backend.deleted_file().key().clone()),
        MutationPreconditions::new(
            Some(backend.deleted_file().metadata_revision().clone()),
            backend
                .deleted_file()
                .content()
                .map(|content| content.revision().clone()),
        ),
    )
}

fn scoped_delete_intent(
    scope: CloudScope,
    item: &str,
    operation: &str,
    idempotency: &str,
    generation: SessionGeneration,
) -> MutationIntent {
    MutationIntent::new(
        operation_id(operation),
        idempotency_key(idempotency),
        MutationOrigin::PlatformCommand,
        generation,
        DesiredMutation::delete(CloudItemKey::new(
            scope,
            CloudItemId::new(item).expect("item fixture should be valid"),
        )),
        MutationPreconditions::default(),
    )
}

#[test]
fn content_mutation_requires_an_owned_immutable_local_content_snapshot() {
    let backend = SyntheticBackend::full();
    let intent = MutationIntent::new(
        operation_id("modify-content"),
        idempotency_key("modify-content-key"),
        MutationOrigin::PlatformObserved,
        generation(1),
        DesiredMutation::modify_content(backend.moved_file().key().clone()),
        MutationPreconditions::default(),
    );
    assert!(intent.validate_for_persistence().is_err());

    let valid = intent.with_local_content(LocalContentSnapshot::new(
        backend.moved_file().key().clone(),
        LocalContentGeneration::new(1).expect("local generation fixture should be valid"),
        LocalContentReference::new("cache-object-42")
            .expect("local content reference fixture should be valid"),
        17,
        None,
    ));
    assert!(valid.validate_for_persistence().is_ok());
}

#[test]
fn metadata_and_content_mutations_keep_independent_revision_preconditions() {
    let backend = SyntheticBackend::full();
    let metadata_only =
        MutationPreconditions::new(Some(backend.moved_file().metadata_revision().clone()), None);
    let content_only = MutationPreconditions::new(
        None,
        backend
            .moved_file()
            .content()
            .map(|content| content.revision().clone()),
    );

    assert!(metadata_only.metadata_revision().is_some());
    assert!(metadata_only.content_revision().is_none());
    assert!(content_only.metadata_revision().is_none());
    assert!(content_only.content_revision().is_some());
}

#[tokio::test]
async fn command_intent_is_durable_before_success_can_be_observed() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "durable-command",
        "durable-command-key",
        session,
        MutationOrigin::PlatformCommand,
    );
    let operation = intent.operation_id().clone();
    store
        .failures()
        .arm(FailurePoint::AfterMutationIntentPersistBeforeReturn);

    let error = store
        .persist_mutation_intent(intent)
        .await
        .expect_err("injected crash should hide the successful store return");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::PersistenceFailure);

    let recovered = store
        .load_mutation(&operation)
        .await
        .expect("mutation recovery load should succeed")
        .expect("intent must already be durable");
    assert_eq!(recovered.state(), MutationState::IntentPersisted);
    assert_eq!(recovered.intent().origin(), MutationOrigin::PlatformCommand);
}

#[tokio::test]
async fn observed_pre_journal_crash_is_rediscovered_instead_of_claimed_durable() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "observed-reconcile",
        "observed-reconcile-key",
        session,
        MutationOrigin::PlatformObserved,
    );
    let operation = intent.operation_id().clone();
    store
        .failures()
        .arm(FailurePoint::BeforeMutationIntentPersist);

    store
        .persist_mutation_intent(intent.clone())
        .await
        .expect_err("pre-journal crash should leave no durable record");
    assert!(
        store
            .load_mutation(&operation)
            .await
            .expect("mutation lookup should succeed")
            .is_none()
    );

    assert_eq!(
        store
            .persist_mutation_intent(intent)
            .await
            .expect("startup reconciliation should persist the rediscovered effect"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn remote_commit_before_outcome_marker_recovers_as_already_committed() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "unknown-outcome",
        "unknown-outcome-key",
        session,
        MutationOrigin::PlatformCommand,
    );
    let operation = intent.operation_id().clone();
    let idempotency = intent.idempotency_key().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent persistence should succeed");
    store
        .begin_remote_apply(&operation, session)
        .await
        .expect("remote apply marker should succeed");

    let mut remote = SyntheticMutationRemote::default();
    let committed = remote.apply(&idempotency);
    assert!(matches!(
        committed,
        MutationRemoteOutcome::Committed { item: None }
    ));
    assert_eq!(
        store
            .load_mutation(&operation)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should remain recoverable")
            .state(),
        MutationState::RemoteApplying
    );

    let recovery_session = generation(2);
    store
        .activate_session(backend.scope(), recovery_session)
        .await
        .expect("restart should activate a newer recovery generation");
    let reconciled = remote
        .reconcile(&idempotency)
        .expect("status reconciliation should find the committed mutation");
    store
        .record_remote_outcome(&operation, recovery_session, reconciled)
        .await
        .expect("already-committed outcome should become durable");
    let recovered = store
        .load_mutation(&operation)
        .await
        .expect("mutation load should succeed")
        .expect("mutation should exist");
    assert_eq!(recovered.state(), MutationState::RemoteOutcomeKnown);
    assert!(matches!(
        recovered.remote_outcome(),
        Some(MutationRemoteOutcome::AlreadyCommitted { item: None })
    ));
    assert_eq!(remote.attempts(&idempotency), 1);
}

#[tokio::test]
async fn explicit_unknown_outcome_blocks_platform_reconciliation_until_resolved() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "explicit-unknown",
        "explicit-unknown-key",
        session,
        MutationOrigin::PlatformPreflight,
    );
    let operation = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent persistence should succeed");
    store
        .begin_remote_apply(&operation, session)
        .await
        .expect("remote apply marker should succeed");
    store
        .record_remote_outcome(
            &operation,
            session,
            MutationRemoteOutcome::RemoteOutcomeUnknown,
        )
        .await
        .expect("unknown outcome should be durable");

    let error = store
        .mark_platform_reconciled(&operation, session)
        .await
        .expect_err("unknown remote outcome must not be presented as reconciled");
    assert_eq!(error.kind(), CloudFilesStoreErrorKind::InvalidTransition);

    store
        .record_remote_outcome(
            &operation,
            session,
            MutationRemoteOutcome::AlreadyCommitted { item: None },
        )
        .await
        .expect("reconciled remote outcome should become known");
    assert_eq!(
        store
            .mark_platform_reconciled(&operation, session)
            .await
            .expect("platform reconciliation should now succeed"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn durable_outcome_and_platform_markers_are_idempotent_after_lost_returns() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "lost-marker-returns",
        "lost-marker-key",
        session,
        MutationOrigin::PlatformCommand,
    );
    let operation = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent persistence should succeed");
    store
        .begin_remote_apply(&operation, session)
        .await
        .expect("remote apply marker should succeed");

    let outcome = MutationRemoteOutcome::Committed { item: None };
    store
        .failures()
        .arm(FailurePoint::AfterRemoteOutcomePersistBeforeReturn);
    store
        .record_remote_outcome(&operation, session, outcome.clone())
        .await
        .expect_err("outcome marker return is intentionally lost");
    assert_eq!(
        store
            .record_remote_outcome(&operation, session, outcome)
            .await
            .expect("duplicate durable outcome should be recognized"),
        StoreWriteStatus::AlreadyApplied
    );

    store
        .failures()
        .arm(FailurePoint::AfterPlatformReconciledPersistBeforeReturn);
    store
        .mark_platform_reconciled(&operation, session)
        .await
        .expect_err("platform marker return is intentionally lost");
    assert_eq!(
        store
            .mark_platform_reconciled(&operation, session)
            .await
            .expect("duplicate platform marker should be recognized"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .complete_mutation(&operation, session)
            .await
            .expect("completion should succeed"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn newer_session_generation_fences_late_completion() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let old = generation(1);
    let new = generation(2);
    store
        .activate_session(backend.scope(), old)
        .await
        .expect("old session activation should succeed");
    let intent = delete_intent(
        &backend,
        "old-session-operation",
        "old-session-key",
        old,
        MutationOrigin::PlatformCommand,
    );
    let operation = intent.operation_id().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("old intent should become durable");
    store
        .begin_remote_apply(&operation, old)
        .await
        .expect("old remote apply should begin");
    store
        .activate_session(backend.scope(), new)
        .await
        .expect("new generation should replace the old one");

    assert_eq!(
        store
            .record_remote_outcome(
                &operation,
                old,
                MutationRemoteOutcome::Committed { item: None },
            )
            .await
            .expect("late completion should be classified"),
        StoreWriteStatus::Fenced
    );
    assert_eq!(
        store
            .load_mutation(&operation)
            .await
            .expect("mutation load should succeed")
            .expect("mutation should exist")
            .state(),
        MutationState::RemoteApplying
    );
}

#[tokio::test]
async fn completed_mutation_replay_does_not_repeat_remote_effect() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    let intent = delete_intent(
        &backend,
        "completed-operation",
        "completed-key",
        session,
        MutationOrigin::PlatformCommand,
    );
    let operation = intent.operation_id().clone();
    let idempotency = intent.idempotency_key().clone();
    store
        .persist_mutation_intent(intent)
        .await
        .expect("intent persistence should succeed");
    store
        .begin_remote_apply(&operation, session)
        .await
        .expect("remote apply marker should succeed");

    let mut remote = SyntheticMutationRemote::default();
    let outcome = remote.apply(&idempotency);
    store
        .record_remote_outcome(&operation, session, outcome.clone())
        .await
        .expect("outcome persistence should succeed");
    store
        .mark_platform_reconciled(&operation, session)
        .await
        .expect("platform reconciliation should succeed");
    store
        .complete_mutation(&operation, session)
        .await
        .expect("completion should succeed");

    let recoverable = store
        .recoverable_mutations(backend.scope())
        .await
        .expect("recovery scan should succeed");
    assert!(recoverable.is_empty());
    assert_eq!(remote.attempts(&idempotency), 1);
    assert_eq!(
        store
            .begin_remote_apply(&operation, session)
            .await
            .expect("late remote-apply marker should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .record_remote_outcome(&operation, session, outcome)
            .await
            .expect("late durable outcome should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .mark_platform_reconciled(&operation, session)
            .await
            .expect("late platform marker should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(
        store
            .complete_mutation(&operation, session)
            .await
            .expect("duplicate completion should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );
    assert_eq!(remote.attempts(&idempotency), 1);
}

#[tokio::test]
async fn session_lifecycle_is_ordered_and_rejects_new_intents_after_closing() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    assert_eq!(
        store
            .transition_session(backend.scope(), session, SessionState::Closing)
            .await
            .expect("closing transition should succeed"),
        StoreWriteStatus::Applied
    );

    let late_intent = delete_intent(
        &backend,
        "late-command",
        "late-command-key",
        session,
        MutationOrigin::PlatformCommand,
    );
    assert_eq!(
        store
            .persist_mutation_intent(late_intent)
            .await
            .expect("late intent should be fenced"),
        StoreWriteStatus::Fenced
    );
    assert_eq!(
        store
            .transition_session(backend.scope(), session, SessionState::Draining)
            .await
            .expect("draining transition should succeed"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .transition_session(backend.scope(), session, SessionState::Closed)
            .await
            .expect("closed transition should succeed"),
        StoreWriteStatus::Applied
    );
}

#[tokio::test]
async fn mutation_intent_requires_active_session_and_rejects_operation_and_idempotency_conflicts() {
    let backend = SyntheticBackend::full();
    let store = MemoryCloudFilesStore::default();
    let session = generation(1);
    let original = delete_intent(
        &backend,
        "operation-a",
        "idempotency-a",
        session,
        MutationOrigin::PlatformCommand,
    );
    assert_eq!(
        store
            .persist_mutation_intent(original.clone())
            .await
            .expect_err("intent without an active session should fail")
            .kind(),
        CloudFilesStoreErrorKind::NotFound
    );
    store
        .activate_session(backend.scope(), session)
        .await
        .expect("session activation should succeed");
    assert_eq!(
        store
            .persist_mutation_intent(original.clone())
            .await
            .expect("first intent should persist"),
        StoreWriteStatus::Applied
    );
    assert_eq!(
        store
            .persist_mutation_intent(original)
            .await
            .expect("same intent should be idempotent"),
        StoreWriteStatus::AlreadyApplied
    );

    let same_operation = delete_intent(
        &backend,
        "operation-a",
        "different-idempotency",
        session,
        MutationOrigin::PlatformObserved,
    );
    assert_eq!(
        store
            .persist_mutation_intent(same_operation)
            .await
            .expect_err("same operation id with another intent should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
    let same_idempotency = delete_intent(
        &backend,
        "operation-b",
        "idempotency-a",
        session,
        MutationOrigin::PlatformCommand,
    );
    assert_eq!(
        store
            .persist_mutation_intent(same_idempotency)
            .await
            .expect_err("same idempotency key with another operation should conflict")
            .kind(),
        CloudFilesStoreErrorKind::Conflict
    );
}

#[tokio::test]
async fn mutation_recovery_is_scope_filtered_sorted_and_excludes_completed_records() {
    let store = MemoryCloudFilesStore::default();
    let scope_a = CloudScope::new(
        CloudNamespaceId::new("namespace-a").expect("namespace fixture should be valid"),
        CloudRootId::new("root-a").expect("root fixture should be valid"),
    );
    let scope_b = CloudScope::new(
        CloudNamespaceId::new("namespace-b").expect("namespace fixture should be valid"),
        CloudRootId::new("root-b").expect("root fixture should be valid"),
    );
    let session = generation(1);
    store
        .activate_session(&scope_a, session)
        .await
        .expect("scope A session should activate");
    store
        .activate_session(&scope_b, session)
        .await
        .expect("scope B session should activate");
    for intent in [
        scoped_delete_intent(scope_a.clone(), "item-z", "z-operation", "idem-z", session),
        scoped_delete_intent(scope_a.clone(), "item-a", "a-operation", "idem-a", session),
        scoped_delete_intent(scope_b.clone(), "item-b", "b-operation", "idem-b", session),
    ] {
        store
            .persist_mutation_intent(intent)
            .await
            .expect("intent should persist");
    }

    assert_eq!(
        store
            .recoverable_mutations(&scope_a)
            .await
            .expect("scope A recovery should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["a-operation", "z-operation"]
    );
    assert_eq!(
        store
            .recoverable_mutations(&scope_b)
            .await
            .expect("scope B recovery should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["b-operation"]
    );

    let completed = operation_id("a-operation");
    store
        .begin_remote_apply(&completed, session)
        .await
        .expect("remote apply should begin");
    store
        .record_remote_outcome(
            &completed,
            session,
            MutationRemoteOutcome::Committed { item: None },
        )
        .await
        .expect("outcome should persist");
    store
        .mark_platform_reconciled(&completed, session)
        .await
        .expect("platform should reconcile");
    store
        .complete_mutation(&completed, session)
        .await
        .expect("mutation should complete");
    assert_eq!(
        store
            .recoverable_mutations(&scope_a)
            .await
            .expect("scope A recovery should succeed")
            .iter()
            .map(|record| record.intent().operation_id().as_str())
            .collect::<Vec<_>>(),
        vec!["z-operation"]
    );
}
