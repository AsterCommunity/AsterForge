use std::time::{Duration, SystemTime};

use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudFilesCoreError, CloudItem, CloudItemId, CloudItemKey, CloudItemKind,
    CloudNamespaceId, CloudRootId, CloudScope, ContentRevision, DesiredMutation, IdempotencyKey,
    LocalContentGeneration, LocalContentReference, LocalContentSnapshot, MetadataRevision,
    MutationIntent, MutationOrigin, MutationPreconditions, MutationRecord,
    MutationRecordTransition, MutationRemoteOutcome, MutationRetryMetadata, MutationState,
    OperationId, PlatformRequestCorrelation, RemoteReconciliationKey, SessionGeneration,
    SessionState,
};

fn scope() -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
        CloudRootId::new("root").expect("root fixture should be valid"),
    )
}

fn key(item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope(),
        CloudItemId::new(item).expect("item fixture should be valid"),
    )
}

fn metadata(value: &[u8]) -> MetadataRevision {
    MetadataRevision::from_slice(value).expect("metadata fixture should be valid")
}

fn revision(value: &[u8]) -> ContentRevision {
    ContentRevision::from_slice(value).expect("content revision fixture should be valid")
}

fn snapshot(item: &str, generation: u64) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        key(item),
        LocalContentGeneration::new(generation).expect("generation fixture should be valid"),
        LocalContentReference::new(format!("local://{item}/{generation}"))
            .expect("reference fixture should be valid"),
        4,
        None,
    )
}

fn intent(operation: &str, item: &str) -> MutationIntent {
    MutationIntent::new(
        OperationId::new(operation).expect("operation fixture should be valid"),
        IdempotencyKey::new(format!("idem-{operation}"))
            .expect("idempotency fixture should be valid"),
        MutationOrigin::PlatformObserved,
        SessionGeneration::new(1).expect("session fixture should be valid"),
        DesiredMutation::modify_content(key(item)),
        MutationPreconditions::new(Some(metadata(b"m1")), Some(revision(b"c1"))),
    )
    .with_local_content(snapshot(item, 1))
}

fn current_file(item: &str, size: u64) -> CloudItem {
    CloudItem::file(
        key(item),
        CloudItemId::new("parent").expect("parent fixture should be valid"),
        format!("{item}.bin"),
        metadata(b"m2"),
        CloudContentMetadata::new(revision(b"c2"), None, size),
    )
    .expect("file fixture should be valid")
}

#[test]
fn mutation_opaque_values_preserve_all_access_paths_and_empty_classification() {
    assert_eq!(
        OperationId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "operation id",
        })
    );
    assert_eq!(
        IdempotencyKey::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "idempotency key",
        })
    );
    assert_eq!(
        PlatformRequestCorrelation::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "platform request correlation",
        })
    );
    assert_eq!(
        RemoteReconciliationKey::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "remote reconciliation key",
        })
    );

    let operation = OperationId::new(" operation ").expect("operation fixture should be valid");
    let idempotency =
        IdempotencyKey::new(" idempotency ").expect("idempotency fixture should be valid");
    let platform = PlatformRequestCorrelation::new(" platform ")
        .expect("platform correlation fixture should be valid");
    let remote = RemoteReconciliationKey::new(" remote ")
        .expect("remote correlation fixture should be valid");
    assert_eq!(operation.as_str(), " operation ");
    assert_eq!(idempotency.as_str(), " idempotency ");
    assert_eq!(platform.as_str(), " platform ");
    assert_eq!(remote.as_str(), " remote ");
    assert_eq!(format!("{operation:?}"), "OperationId { byte_len: 11 }");
    assert_eq!(
        format!("{idempotency:?}"),
        "IdempotencyKey { byte_len: 13 }"
    );
    assert_eq!(
        format!("{platform:?}"),
        "PlatformRequestCorrelation { byte_len: 10 }"
    );
    assert_eq!(
        format!("{remote:?}"),
        "RemoteReconciliationKey { byte_len: 8 }"
    );
    assert_eq!(operation.into_string(), " operation ");
    assert_eq!(idempotency.into_string(), " idempotency ");
    assert_eq!(platform.into_string(), " platform ");
    assert_eq!(remote.into_string(), " remote ");
}

#[test]
fn session_generation_and_lifecycle_cover_every_transition_pair() {
    assert_eq!(
        SessionGeneration::new(0),
        Err(CloudFilesCoreError::InvalidSessionGeneration)
    );
    assert_eq!(
        SessionGeneration::new(u64::MAX)
            .expect("maximum generation should be valid")
            .get(),
        u64::MAX
    );

    let states = [
        SessionState::Accepting,
        SessionState::Closing,
        SessionState::Draining,
        SessionState::Closed,
    ];
    for current in states {
        for next in states {
            let expected = matches!(
                (current, next),
                (
                    SessionState::Accepting,
                    SessionState::Accepting | SessionState::Closing
                ) | (
                    SessionState::Closing,
                    SessionState::Closing | SessionState::Draining
                ) | (
                    SessionState::Draining,
                    SessionState::Draining | SessionState::Closed
                ) | (SessionState::Closed, SessionState::Closed)
            );
            assert_eq!(
                current.can_transition_to(next),
                expected,
                "{current:?} -> {next:?}"
            );
        }
    }
}

#[test]
fn desired_mutation_validates_names_and_reports_scope_and_existing_target() {
    assert_eq!(
        DesiredMutation::create(
            scope(),
            CloudItemId::new("parent").expect("parent fixture should be valid"),
            "",
            CloudItemKind::File,
        ),
        Err(CloudFilesCoreError::EmptyValue {
            field: "mutation create name",
        })
    );
    let create = DesiredMutation::create(
        scope(),
        CloudItemId::new("parent").expect("parent fixture should be valid"),
        " name ",
        CloudItemKind::Directory,
    )
    .expect("create fixture should be valid");
    assert_eq!(create.scope(), &scope());
    assert!(create.existing_item_key().is_none());

    let item_key = key("item");
    assert_eq!(
        DesiredMutation::modify_metadata(item_key.clone(), None, None),
        Err(CloudFilesCoreError::InvalidMutationIntent {
            reason: "metadata mutation must change parent or name",
        })
    );
    assert_eq!(
        DesiredMutation::modify_metadata(item_key.clone(), None, Some(String::new())),
        Err(CloudFilesCoreError::EmptyValue {
            field: "mutation metadata name",
        })
    );

    let variants = [
        DesiredMutation::modify_content(item_key.clone()),
        DesiredMutation::modify_metadata(
            item_key.clone(),
            Some(CloudItemId::new("new-parent").expect("parent fixture should be valid")),
            Some("new-name".to_owned()),
        )
        .expect("metadata fixture should be valid"),
        DesiredMutation::delete(item_key.clone()),
    ];
    for desired in variants {
        assert_eq!(desired.scope(), item_key.scope());
        assert_eq!(desired.existing_item_key(), Some(&item_key));
    }
}

#[test]
fn mutation_intent_exposes_every_owned_boundary_value() {
    let retry_at = SystemTime::UNIX_EPOCH + Duration::from_secs(123);
    let retry = MutationRetryMetadata::new(u32::MAX, Some(retry_at));
    assert_eq!(retry.attempt(), u32::MAX);
    assert_eq!(retry.not_before(), Some(retry_at));
    assert_eq!(MutationRetryMetadata::default().attempt(), 0);
    assert_eq!(MutationRetryMetadata::default().not_before(), None);

    let local = snapshot("item", 2);
    let intent = MutationIntent::new(
        OperationId::new("operation").expect("operation fixture should be valid"),
        IdempotencyKey::new("idempotency").expect("idempotency fixture should be valid"),
        MutationOrigin::PlatformPreflight,
        SessionGeneration::new(9).expect("session fixture should be valid"),
        DesiredMutation::modify_content(key("item")),
        MutationPreconditions::new(Some(metadata(b"m1")), Some(revision(b"c1"))),
    )
    .with_local_content(local.clone())
    .with_platform_correlation(
        PlatformRequestCorrelation::new("platform").expect("correlation fixture should be valid"),
    )
    .with_remote_reconciliation_key(
        RemoteReconciliationKey::new("remote").expect("reconciliation fixture should be valid"),
    )
    .with_retry(retry.clone());

    intent
        .validate_for_persistence()
        .expect("complete intent should validate");
    assert_eq!(intent.operation_id().as_str(), "operation");
    assert_eq!(intent.idempotency_key().as_str(), "idempotency");
    assert_eq!(intent.origin(), MutationOrigin::PlatformPreflight);
    assert_eq!(intent.session_generation().get(), 9);
    assert!(matches!(
        intent.desired(),
        DesiredMutation::ModifyContent { .. }
    ));
    assert_eq!(
        intent
            .preconditions()
            .metadata_revision()
            .map(MetadataRevision::as_bytes),
        Some(&b"m1"[..])
    );
    assert_eq!(
        intent
            .preconditions()
            .content_revision()
            .map(ContentRevision::as_bytes),
        Some(&b"c1"[..])
    );
    assert_eq!(intent.local_content(), Some(&local));
    assert_eq!(
        intent
            .platform_correlation()
            .map(PlatformRequestCorrelation::as_str),
        Some("platform")
    );
    assert_eq!(
        intent
            .remote_reconciliation_key()
            .map(RemoteReconciliationKey::as_str),
        Some("remote")
    );
    assert_eq!(intent.retry(), &retry);
}

#[test]
fn content_intent_rejects_missing_or_mismatched_local_snapshot() {
    let missing = MutationIntent::new(
        OperationId::new("missing").expect("operation fixture should be valid"),
        IdempotencyKey::new("missing").expect("idempotency fixture should be valid"),
        MutationOrigin::PlatformCommand,
        SessionGeneration::new(1).expect("session fixture should be valid"),
        DesiredMutation::modify_content(key("item")),
        MutationPreconditions::default(),
    );
    assert_eq!(
        missing.validate_for_persistence(),
        Err(CloudFilesCoreError::InvalidMutationIntent {
            reason: "content mutation requires a local content reference",
        })
    );

    let mismatched = missing.with_local_content(snapshot("other-item", 1));
    assert_eq!(
        mismatched.validate_for_persistence(),
        Err(CloudFilesCoreError::InvalidMutationIntent {
            reason: "local content snapshot item does not match the mutation target",
        })
    );
}

#[test]
fn non_content_mutations_reject_local_content_snapshots() {
    let desired = [
        DesiredMutation::create(
            scope(),
            CloudItemId::new("parent").expect("parent fixture should be valid"),
            "file.bin",
            CloudItemKind::File,
        )
        .expect("create fixture should be valid"),
        DesiredMutation::modify_metadata(key("item"), None, Some("renamed".to_owned()))
            .expect("metadata fixture should be valid"),
        DesiredMutation::delete(key("item")),
    ];
    for (index, desired) in desired.into_iter().enumerate() {
        let intent = MutationIntent::new(
            OperationId::new(format!("operation-{index}"))
                .expect("operation fixture should be valid"),
            IdempotencyKey::new(format!("idempotency-{index}"))
                .expect("idempotency fixture should be valid"),
            MutationOrigin::PlatformObserved,
            SessionGeneration::new(1).expect("session fixture should be valid"),
            desired,
            MutationPreconditions::default(),
        )
        .with_local_content(snapshot("item", 1));
        assert_eq!(
            intent.validate_for_persistence(),
            Err(CloudFilesCoreError::InvalidMutationIntent {
                reason: "local content snapshot is only valid for content mutation",
            })
        );
    }
}

#[test]
fn mutation_record_rejects_every_transition_before_its_prerequisite() {
    let mut record = MutationRecord::persist(intent("operation", "item"))
        .expect("record fixture should be valid");
    assert_eq!(record.state(), MutationState::IntentPersisted);
    assert!(record.remote_outcome().is_none());
    assert_eq!(record.intent().operation_id().as_str(), "operation");
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Err(CloudFilesCoreError::InvalidMutationTransition {
            reason: "remote outcome must follow remote apply or reconcile an unknown outcome",
        })
    );
    assert_eq!(
        record.mark_platform_reconciled(),
        Err(CloudFilesCoreError::InvalidMutationTransition {
            reason: "platform reconciliation requires a known remote outcome",
        })
    );
    assert_eq!(
        record.complete(),
        Err(CloudFilesCoreError::InvalidMutationTransition {
            reason: "completion requires platform reconciliation",
        })
    );
}

#[test]
fn mutation_record_unknown_outcome_can_repeat_and_then_resolve_once() {
    let mut record = MutationRecord::persist(intent("operation", "item"))
        .expect("record fixture should be valid");
    assert_eq!(
        record.begin_remote_apply(),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(
        record.begin_remote_apply(),
        Ok(MutationRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(record.state(), MutationState::RemoteOutcomeUnknown);
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Ok(MutationRecordTransition::AlreadyApplied)
    );

    let outcome = MutationRemoteOutcome::AlreadyCommitted {
        item: Some(current_file("item", 4)),
    };
    assert_eq!(
        record.record_remote_outcome(outcome.clone()),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(record.state(), MutationState::RemoteOutcomeKnown);
    assert_eq!(record.remote_outcome(), Some(&outcome));
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Err(CloudFilesCoreError::InvalidMutationTransition {
            reason: "remote outcome must follow remote apply or reconcile an unknown outcome",
        })
    );
}

#[test]
fn every_known_remote_outcome_reaches_the_known_state() {
    let outcomes = [
        MutationRemoteOutcome::Committed {
            item: Some(current_file("item", 4)),
        },
        MutationRemoteOutcome::AlreadyCommitted { item: None },
        MutationRemoteOutcome::PreconditionFailed {
            metadata_revision: Some(metadata(b"m-current")),
            content_revision: Some(revision(b"c-current")),
        },
    ];

    for (index, outcome) in outcomes.into_iter().enumerate() {
        let mut record = MutationRecord::persist(intent(&format!("operation-{index}"), "item"))
            .expect("record fixture should be valid");
        record
            .begin_remote_apply()
            .expect("remote apply should begin");
        assert_eq!(
            record.record_remote_outcome(outcome.clone()),
            Ok(MutationRecordTransition::Applied)
        );
        assert_eq!(record.state(), MutationState::RemoteOutcomeKnown);
        assert_eq!(record.remote_outcome(), Some(&outcome));
    }
}

#[test]
fn completed_mutation_accepts_late_replay_of_already_durable_markers() {
    let outcome = MutationRemoteOutcome::Committed {
        item: Some(current_file("item", 4)),
    };
    let mut record = MutationRecord::persist(intent("operation", "item"))
        .expect("record fixture should be valid");
    assert_eq!(
        record.begin_remote_apply(),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(
        record.record_remote_outcome(outcome.clone()),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_platform_reconciled(),
        Ok(MutationRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_platform_reconciled(),
        Ok(MutationRecordTransition::AlreadyApplied)
    );
    assert_eq!(record.complete(), Ok(MutationRecordTransition::Applied));
    assert_eq!(
        record.complete(),
        Ok(MutationRecordTransition::AlreadyApplied)
    );

    assert_eq!(
        record.begin_remote_apply(),
        Ok(MutationRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_remote_outcome(outcome),
        Ok(MutationRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.mark_platform_reconciled(),
        Ok(MutationRecordTransition::AlreadyApplied)
    );
}

#[test]
fn completed_mutation_rejects_a_different_late_remote_outcome() {
    let committed = MutationRemoteOutcome::Committed {
        item: Some(current_file("item", 4)),
    };
    let mut record = MutationRecord::persist(intent("operation", "item"))
        .expect("record fixture should be valid");
    record
        .begin_remote_apply()
        .expect("remote apply should begin");
    record
        .record_remote_outcome(committed)
        .expect("outcome should be recorded");
    record
        .mark_platform_reconciled()
        .expect("platform should reconcile");
    record.complete().expect("record should complete");

    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::PreconditionFailed {
            metadata_revision: None,
            content_revision: None,
        }),
        Err(CloudFilesCoreError::InvalidMutationTransition {
            reason: "remote outcome must follow remote apply or reconcile an unknown outcome",
        })
    );
}
