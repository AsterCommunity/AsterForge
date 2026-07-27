use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudFilesCoreError, CloudItem, CloudItemId, CloudItemKey,
    CloudNamespaceId, CloudRootId, CloudScope, ContentCacheKey, ContentLeaseId, ContentRevision,
    ContentUploadChunk, ContentUploadChunkAck, ContentUploadIntent, ContentUploadRecord,
    ContentUploadRecordTransition, ContentUploadSession, ContentUploadSessionId,
    ContentUploadState, IdempotencyKey, LocalContentGeneration, LocalContentReference,
    LocalContentSnapshot, MetadataRevision, MutationRemoteOutcome, OperationId, SessionGeneration,
};
use bytes::Bytes;

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

fn revision(value: &[u8]) -> ContentRevision {
    ContentRevision::from_slice(value).expect("revision fixture should be valid")
}

fn snapshot(item: &str, generation: u64, size: u64) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        key(item),
        LocalContentGeneration::new(generation).expect("generation fixture should be valid"),
        LocalContentReference::new(format!("local://{item}/{generation}"))
            .expect("reference fixture should be valid"),
        size,
        None,
    )
}

fn upload_intent(item: &str, generation: u64, size: u64) -> ContentUploadIntent {
    ContentUploadIntent::new(
        OperationId::new(format!("upload-{item}-{generation}"))
            .expect("operation fixture should be valid"),
        IdempotencyKey::new(format!("idem-{item}-{generation}"))
            .expect("idempotency fixture should be valid"),
        ContentCacheKey::new(key(item), revision(b"base-c1")),
        snapshot(item, generation, size),
        ContentLeaseId::new(format!("lease-{item}-{generation}"))
            .expect("lease fixture should be valid"),
        SessionGeneration::new(3).expect("session fixture should be valid"),
    )
    .expect("upload intent fixture should be valid")
}

fn session(id: &str, accepted_offset: u64) -> ContentUploadSession {
    ContentUploadSession::new(
        ContentUploadSessionId::new(id).expect("upload session fixture should be valid"),
        accepted_offset,
    )
}

fn file(item: &str, size: u64) -> CloudItem {
    CloudItem::file(
        key(item),
        CloudItemId::new("parent").expect("parent fixture should be valid"),
        format!("{item}.bin"),
        MetadataRevision::from_slice(b"m2").expect("metadata fixture should be valid"),
        CloudContentMetadata::new(revision(b"uploaded-c2"), None, size),
    )
    .expect("file fixture should be valid")
}

#[test]
fn upload_session_id_and_intent_accessors_preserve_exact_values() {
    assert_eq!(
        ContentUploadSessionId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "content upload session id",
        })
    );
    let session_id =
        ContentUploadSessionId::new(" session/保留 ").expect("session fixture should be valid");
    assert_eq!(session_id.as_str(), " session/保留 ");
    assert_eq!(
        format!("{session_id:?}"),
        "ContentUploadSessionId { byte_len: 16 }"
    );

    let intent = upload_intent("file", 2, 8);
    assert_eq!(intent.operation_id().as_str(), "upload-file-2");
    assert_eq!(intent.idempotency_key().as_str(), "idem-file-2");
    assert_eq!(intent.base_cache_key().item_key(), &key("file"));
    assert_eq!(intent.base_cache_key().revision().as_bytes(), b"base-c1");
    assert_eq!(intent.snapshot().generation().get(), 2);
    assert_eq!(intent.snapshot().size(), 8);
    assert_eq!(intent.upload_lease_id().as_str(), "lease-file-2");
    assert_eq!(intent.session_generation().get(), 3);
}

#[test]
fn upload_intent_rejects_snapshot_for_another_item() {
    assert_eq!(
        ContentUploadIntent::new(
            OperationId::new("operation").expect("operation fixture should be valid"),
            IdempotencyKey::new("idempotency").expect("idempotency fixture should be valid"),
            ContentCacheKey::new(key("file"), revision(b"base-c1")),
            snapshot("other", 1, 4),
            ContentLeaseId::new("lease").expect("lease fixture should be valid"),
            SessionGeneration::new(1).expect("session fixture should be valid"),
        ),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload snapshot item does not match the base cache key",
        })
    );
}

#[test]
fn upload_session_checkpoint_must_remain_within_snapshot() {
    let intent = upload_intent("file", 1, 4);
    let valid = session("session", 4);
    assert_eq!(valid.id().as_str(), "session");
    assert_eq!(valid.accepted_offset(), 4);
    valid
        .validate_for(&intent)
        .expect("offset at snapshot end should validate");
    assert_eq!(
        session("session", 5).validate_for(&intent),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "backend upload offset exceeds the immutable snapshot size",
        })
    );
}

#[test]
fn upload_chunk_constructor_rejects_empty_overflow_and_snapshot_overrun() {
    let session_id =
        ContentUploadSessionId::new("session").expect("session fixture should be valid");
    let generation = LocalContentGeneration::new(1).expect("generation fixture should be valid");
    assert_eq!(
        ContentUploadChunk::new(session_id.clone(), generation, 0, Bytes::new(), 0),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunks must contain at least one byte",
        })
    );
    assert_eq!(
        ContentUploadChunk::new(
            session_id.clone(),
            generation,
            u64::MAX,
            Bytes::from_static(b"x"),
            u64::MAX,
        ),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk end exceeds u64",
        })
    );
    assert_eq!(
        ContentUploadChunk::new(session_id, generation, 3, Bytes::from_static(b"xx"), 4,),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk exceeds the immutable snapshot size",
        })
    );
}

#[test]
fn upload_chunk_accessors_and_validation_cover_every_binding() {
    let intent = upload_intent("file", 2, 8);
    let durable_session = session("session", 2);
    let chunk = ContentUploadChunk::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        LocalContentGeneration::new(2).expect("generation fixture should be valid"),
        2,
        Bytes::from_static(b"abcd"),
        8,
    )
    .expect("chunk fixture should be valid");
    assert_eq!(chunk.session_id().as_str(), "session");
    assert_eq!(chunk.local_generation().get(), 2);
    assert_eq!(chunk.offset(), 2);
    assert_eq!(chunk.bytes().as_ref(), b"abcd");
    assert_eq!(chunk.byte_len(), 4);
    assert_eq!(chunk.end_exclusive(), 6);
    assert_eq!(chunk.total_size(), 8);
    chunk
        .validate_for(&intent, &durable_session)
        .expect("exact chunk binding should validate");

    let wrong_session = ContentUploadChunk::new(
        ContentUploadSessionId::new("other").expect("session fixture should be valid"),
        LocalContentGeneration::new(2).expect("generation fixture should be valid"),
        2,
        Bytes::from_static(b"abcd"),
        8,
    )
    .expect("chunk fixture should be valid");
    assert_eq!(
        wrong_session.validate_for(&intent, &durable_session),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk belongs to another backend session",
        })
    );

    let wrong_generation = ContentUploadChunk::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        LocalContentGeneration::new(1).expect("generation fixture should be valid"),
        2,
        Bytes::from_static(b"abcd"),
        8,
    )
    .expect("chunk fixture should be valid");
    assert_eq!(
        wrong_generation.validate_for(&intent, &durable_session),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk belongs to another local generation",
        })
    );

    let wrong_total = ContentUploadChunk::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        LocalContentGeneration::new(2).expect("generation fixture should be valid"),
        2,
        Bytes::from_static(b"abcd"),
        9,
    )
    .expect("chunk fixture should be valid");
    assert_eq!(
        wrong_total.validate_for(&intent, &durable_session),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk size does not match the immutable snapshot",
        })
    );

    let wrong_offset = ContentUploadChunk::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        LocalContentGeneration::new(2).expect("generation fixture should be valid"),
        1,
        Bytes::from_static(b"abcd"),
        8,
    )
    .expect("chunk fixture should be valid");
    assert_eq!(
        wrong_offset.validate_for(&intent, &durable_session),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload chunk does not continue the durable accepted offset",
        })
    );
}

#[test]
fn upload_acknowledgement_must_match_session_and_exact_chunk_end() {
    let chunk = ContentUploadChunk::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        LocalContentGeneration::new(1).expect("generation fixture should be valid"),
        2,
        Bytes::from_static(b"abcd"),
        8,
    )
    .expect("chunk fixture should be valid");
    let exact = ContentUploadChunkAck::new(
        ContentUploadSessionId::new("session").expect("session fixture should be valid"),
        6,
    );
    assert_eq!(exact.session_id().as_str(), "session");
    assert_eq!(exact.accepted_offset(), 6);
    exact
        .validate_for(&chunk)
        .expect("exact acknowledgement should validate");
    assert_eq!(
        ContentUploadChunkAck::new(
            ContentUploadSessionId::new("other").expect("session fixture should be valid"),
            6,
        )
        .validate_for(&chunk),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload acknowledgement belongs to another session",
        })
    );
    assert_eq!(
        ContentUploadChunkAck::new(
            ContentUploadSessionId::new("session").expect("session fixture should be valid"),
            5,
        )
        .validate_for(&chunk),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload acknowledgement does not match the submitted chunk end",
        })
    );
}

#[test]
fn upload_record_session_offsets_are_monotonic_and_session_identity_is_stable() {
    let intent = upload_intent("file", 1, 8);
    let mut record = ContentUploadRecord::persist(intent.clone());
    assert_eq!(record.intent(), &intent);
    assert_eq!(record.state(), ContentUploadState::IntentPersisted);
    assert!(record.session().is_none());
    assert!(record.remote_outcome().is_none());

    assert_eq!(
        record.record_session(session("session", 2)),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(record.state(), ContentUploadState::Uploading);
    assert_eq!(
        record.session().map(ContentUploadSession::accepted_offset),
        Some(2)
    );
    assert_eq!(
        record.record_session(session("session", 2)),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_session(session("session", 1)),
        Ok(ContentUploadRecordTransition::Fenced)
    );
    assert_eq!(
        record.record_session(session("other", 3)),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "one upload operation cannot switch backend session identity",
        })
    );
    assert_eq!(
        record.record_session(session("session", 8)),
        Ok(ContentUploadRecordTransition::Applied)
    );
}

#[test]
fn remote_commit_requires_a_session_and_complete_bytes() {
    let mut no_session = ContentUploadRecord::persist(upload_intent("file", 1, 8));
    assert_eq!(
        no_session.begin_remote_commit(),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "remote commit requires a backend upload session",
        })
    );

    let mut partial = ContentUploadRecord::persist(upload_intent("file", 1, 8));
    partial
        .record_session(session("session", 7))
        .expect("session should persist");
    assert_eq!(
        partial.begin_remote_commit(),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "remote commit requires every immutable byte to be accepted",
        })
    );
}

#[test]
fn empty_file_upload_can_commit_at_offset_zero() {
    let mut record = ContentUploadRecord::persist(upload_intent("empty", 1, 0));
    assert_eq!(
        record.record_session(session("session", 0)),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(
        record.begin_remote_commit(),
        Ok(ContentUploadRecordTransition::Applied)
    );
}

#[test]
fn remote_outcome_cannot_precede_commit_and_unknown_may_resolve_once() {
    let mut record = ContentUploadRecord::persist(upload_intent("file", 1, 4));
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "remote upload outcome must follow commit or reconcile an unknown outcome",
        })
    );
    record
        .record_session(session("session", 4))
        .expect("session should persist");
    assert_eq!(
        record.begin_remote_commit(),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(
        record.begin_remote_commit(),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::RemoteOutcomeUnknown),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::PreconditionFailed {
            metadata_revision: None,
            content_revision: None,
        }),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(record.state(), ContentUploadState::RemoteOutcomeKnown);
}

#[test]
fn committed_upload_outcome_requires_matching_file_metadata_and_size() {
    let mut record = ContentUploadRecord::persist(upload_intent("file", 1, 4));
    record
        .record_session(session("session", 4))
        .expect("session should persist");
    record
        .begin_remote_commit()
        .expect("remote commit should begin");

    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::Committed { item: None }),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "committed upload outcome requires current item metadata",
        })
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::Committed {
            item: Some(file("other", 4)),
        }),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "committed upload outcome belongs to another item",
        })
    );
    let directory = CloudItem::directory(
        key("file"),
        Some(CloudItemId::new("parent").expect("parent fixture should be valid")),
        "file",
        MetadataRevision::from_slice(b"m2").expect("metadata fixture should be valid"),
    )
    .expect("directory fixture should be valid");
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::AlreadyCommitted {
            item: Some(directory),
        }),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "committed upload outcome must describe file content",
        })
    );
    assert_eq!(
        record.record_remote_outcome(MutationRemoteOutcome::Committed {
            item: Some(file("file", 5)),
        }),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "committed upload size does not match the immutable snapshot",
        })
    );

    let outcome = MutationRemoteOutcome::AlreadyCommitted {
        item: Some(file("file", 4)),
    };
    assert_eq!(
        record.record_remote_outcome(outcome.clone()),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(record.remote_outcome(), Some(&outcome));
}

#[test]
fn upload_metadata_and_completion_require_order_and_accept_late_replay() {
    let outcome = MutationRemoteOutcome::Committed {
        item: Some(file("file", 4)),
    };
    let mut record = ContentUploadRecord::persist(upload_intent("file", 1, 4));
    assert_eq!(
        record.mark_metadata_reconciled(),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload metadata reconciliation requires a known remote outcome",
        })
    );
    assert_eq!(
        record.complete(),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload completion requires metadata reconciliation",
        })
    );
    record
        .record_session(session("session", 4))
        .expect("session should persist");
    record
        .begin_remote_commit()
        .expect("remote commit should begin");
    record
        .record_remote_outcome(outcome.clone())
        .expect("outcome should persist");
    assert_eq!(
        record.mark_metadata_reconciled(),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(
        record.mark_metadata_reconciled(),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentUploadRecordTransition::Applied)
    );
    assert_eq!(
        record.complete(),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.begin_remote_commit(),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_session(session("session", 4)),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.record_remote_outcome(outcome),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
    assert_eq!(
        record.mark_metadata_reconciled(),
        Ok(ContentUploadRecordTransition::AlreadyApplied)
    );
}

#[test]
fn upload_record_rejects_session_checkpoint_changes_after_commit() {
    let mut record = ContentUploadRecord::persist(upload_intent("file", 1, 4));
    record
        .record_session(session("session", 4))
        .expect("session should persist");
    record
        .begin_remote_commit()
        .expect("remote commit should begin");
    assert_eq!(
        record.record_session(session("session", 3)),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload session checkpoint must precede remote commit",
        })
    );
    assert_eq!(
        record.record_session(session("other", 4)),
        Err(CloudFilesCoreError::InvalidContentUploadTransition {
            reason: "upload session checkpoint must precede remote commit",
        })
    );
}
