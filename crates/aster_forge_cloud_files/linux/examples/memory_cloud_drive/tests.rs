#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("aster-forge-cloud-files-{name}-{suffix}"))
    }

    async fn accepted_create(
        store: &Arc<MemoryWritebackStore>,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> LinuxCreateFileAcceptance {
        store.activate_session(scope, generation).await.unwrap();
        let parent = key(scope, "root").unwrap();
        let request = LinuxCreateFileRequest::new(
            parent,
            "created.txt",
            0o100644,
            0,
            aster_forge_cloud_files_linux::LinuxFileAccess::ReadWrite,
            generation,
        )
        .unwrap();
        store.create_file(&request).await.unwrap()
    }

    async fn write_created(
        store: &MemoryWritebackStore,
        acceptance: &LinuxCreateFileAcceptance,
        bytes: &'static [u8],
    ) -> LinuxWriteCommit {
        store
            .write(acceptance.session().id(), 0, Bytes::from_static(bytes))
            .await
            .unwrap()
    }

    fn upload_operation(snapshot: &LocalContentSnapshot) -> OperationId {
        OperationId::new(format!(
            "content-upload-op-{:020}",
            snapshot.generation().get()
        ))
        .unwrap()
    }

    fn committed_upload_bytes(
        backend: &MemoryRemoteMutationBackend,
        intent: &ContentUploadIntent,
    ) -> Option<Bytes> {
        lock(&backend.state)
            .uploads
            .get(intent.idempotency_key())
            .and_then(|upload| upload.committed.as_ref())
            .map(|(_, bytes)| bytes.clone())
    }

    #[tokio::test]
    async fn write_acceptance_uploads_exact_immutable_bytes_and_clears_current_dirty_state() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let commit = write_created(&store, &acceptance, b"0123456789abcdefg").await;
        let operation_id = upload_operation(commit.snapshot());
        let record = store
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.state(), ContentUploadState::IntentPersisted);
        assert_eq!(
            store.read_snapshot(commit.snapshot(), 3, 9).await.unwrap(),
            Bytes::from_static(b"3456789ab")
        );

        let backend = MemoryRemoteMutationBackend::new(store.clone(), None).unwrap();
        let outcome = ContentUploadRunner::new(8)
            .unwrap()
            .resume(&operation_id, generation, &*store, &backend, &*store)
            .await
            .unwrap();
        assert_eq!(outcome, ContentUploadRunOutcome::Completed);
        let completed = store
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.state(), ContentUploadState::Completed);
        assert_eq!(
            completed
                .session()
                .map(ContentUploadSession::accepted_offset),
            Some(17)
        );
        assert_eq!(
            committed_upload_bytes(&backend, completed.intent()),
            Some(Bytes::from_static(b"0123456789abcdefg"))
        );
        assert!(
            lock(&store.state)
                .files
                .get(acceptance.session().key())
                .unwrap()
                .snapshot
                .is_none()
        );
    }

    #[tokio::test]
    async fn remote_chunk_reconciliation_requires_the_exact_accepted_bytes() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let commit = write_created(&store, &acceptance, b"chunk-reconcile").await;
        let operation_id = upload_operation(commit.snapshot());
        let record = store
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        let backend = MemoryRemoteMutationBackend::new(store.clone(), None).unwrap();
        let session = backend.start_upload(record.intent()).await.unwrap();
        let chunk = ContentUploadChunk::new(
            session.id().clone(),
            record.intent().snapshot().generation(),
            0,
            Bytes::from_static(b"chunk-reconcile"),
            record.intent().snapshot().size(),
        )
        .unwrap();
        backend.upload_chunk(record.intent(), &chunk).await.unwrap();
        assert_eq!(
            backend
                .reconcile_upload_chunk(record.intent(), &session, &chunk)
                .await
                .unwrap()
                .accepted_offset(),
            chunk.end_exclusive()
        );
        let wrong = ContentUploadChunk::new(
            session.id().clone(),
            record.intent().snapshot().generation(),
            0,
            Bytes::from_static(b"wrong-reconcile"),
            record.intent().snapshot().size(),
        )
        .unwrap();
        assert_eq!(
            backend
                .reconcile_upload_chunk(record.intent(), &session, &wrong)
                .await
                .unwrap_err()
                .kind(),
            CloudBackendErrorKind::PreconditionFailed
        );
    }

    #[tokio::test]
    async fn completed_upload_history_and_immutable_generations_are_bounded() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let backend = MemoryRemoteMutationBackend::new(store.clone(), None).unwrap();
        let runner = ContentUploadRunner::new(8).unwrap();

        for value in 0..(TERMINAL_RECORD_LIMIT + 3) {
            let commit = store
                .write(
                    acceptance.session().id(),
                    0,
                    Bytes::from(vec![u8::try_from(value % 251).unwrap(); 32]),
                )
                .await
                .unwrap();
            let operation_id = upload_operation(commit.snapshot());
            assert_eq!(
                runner
                    .resume(&operation_id, generation, &*store, &backend, &*store)
                    .await
                    .unwrap(),
                ContentUploadRunOutcome::Completed
            );
        }

        let state = lock(&store.state);
        assert_eq!(state.uploads.len(), TERMINAL_RECORD_LIMIT);
        assert_eq!(state.upload_idempotency_keys.len(), TERMINAL_RECORD_LIMIT);
        assert_eq!(state.immutable_snapshots.len(), TERMINAL_RECORD_LIMIT);
        assert!(
            state
                .uploads
                .values()
                .all(|record| { record.state() == ContentUploadState::Completed })
        );
    }

    #[tokio::test]
    async fn recovery_pages_are_ordered_bounded_and_cursor_driven() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        for value in [b'a', b'b', b'c'] {
            store
                .write(acceptance.session().id(), 0, Bytes::from(vec![value; 4]))
                .await
                .unwrap();
        }

        let first = store
            .recoverable_content_uploads_page(&scope, None, 2)
            .await
            .unwrap();
        assert_eq!(first.items().len(), 2);
        assert!(first.has_more());
        let cursor = first.items()[1].intent().operation_id().as_str().to_owned();
        let second = store
            .recoverable_content_uploads_page(&scope, Some(&cursor), 2)
            .await
            .unwrap();
        assert_eq!(second.items().len(), 1);
        assert!(!second.has_more());
        assert!(second.items()[0].intent().operation_id().as_str() > cursor.as_str());
        assert_eq!(
            store
                .recoverable_content_uploads_page(&scope, None, 0)
                .await
                .unwrap_err()
                .kind(),
            CloudFilesStoreErrorKind::InvalidTransition
        );
        assert_eq!(
            store
                .recoverable_mutations_page(&scope, None, 0)
                .await
                .unwrap_err()
                .kind(),
            CloudFilesStoreErrorKind::InvalidTransition
        );
    }

    #[tokio::test]
    async fn older_upload_completion_preserves_newer_dirty_generation_and_immutable_bytes() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let first = write_created(&store, &acceptance, b"first-generation").await;
        let second = store
            .write(
                acceptance.session().id(),
                0,
                Bytes::from_static(b"second-generation"),
            )
            .await
            .unwrap();
        let first_operation = upload_operation(first.snapshot());
        let second_operation = upload_operation(second.snapshot());
        let backend = MemoryRemoteMutationBackend::new(store.clone(), None).unwrap();

        assert_eq!(
            ContentUploadRunner::new(5)
                .unwrap()
                .resume(&first_operation, generation, &*store, &backend, &*store,)
                .await
                .unwrap(),
            ContentUploadRunOutcome::Completed
        );
        let dirty = lock(&store.state)
            .files
            .get(acceptance.session().key())
            .and_then(|file| file.snapshot.clone())
            .unwrap();
        assert_eq!(dirty.generation(), second.snapshot().generation());
        assert_eq!(
            store
                .read_snapshot(first.snapshot(), 0, first.snapshot().size())
                .await
                .unwrap(),
            Bytes::from_static(b"first-generation")
        );

        assert_eq!(
            ContentUploadRunner::new(5)
                .unwrap()
                .resume(&second_operation, generation, &*store, &backend, &*store,)
                .await
                .unwrap(),
            ContentUploadRunOutcome::Completed
        );
        assert!(
            lock(&store.state)
                .files
                .get(acceptance.session().key())
                .unwrap()
                .snapshot
                .is_none()
        );
    }

    #[tokio::test]
    async fn remote_upload_commit_before_local_outcome_recovers_after_restart() {
        let directory = test_directory("upload-restart");
        fs::create_dir_all(&directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation_one = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let acceptance = accepted_create(&store, &scope, generation_one).await;
        let commit = write_created(&store, &acceptance, b"restart-upload-bytes").await;
        let operation_id = upload_operation(commit.snapshot());
        let record = store
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        let backend = MemoryRemoteMutationBackend::new(store.clone(), Some(&directory)).unwrap();
        let session = backend.start_upload(record.intent()).await.unwrap();
        store
            .record_content_upload_session(&operation_id, session.clone(), generation_one)
            .await
            .unwrap();
        let bytes = store
            .read_snapshot(
                record.intent().snapshot(),
                0,
                record.intent().snapshot().size(),
            )
            .await
            .unwrap();
        let chunk = ContentUploadChunk::new(
            session.id().clone(),
            record.intent().snapshot().generation(),
            0,
            bytes,
            record.intent().snapshot().size(),
        )
        .unwrap();
        let acknowledgement = backend.upload_chunk(record.intent(), &chunk).await.unwrap();
        let complete_session = ContentUploadSession::new(
            acknowledgement.session_id().clone(),
            acknowledgement.accepted_offset(),
        );
        store
            .record_content_upload_session(&operation_id, complete_session.clone(), generation_one)
            .await
            .unwrap();
        store
            .begin_content_upload_remote_commit(&operation_id, generation_one)
            .await
            .unwrap();
        assert!(matches!(
            backend
                .commit_upload(record.intent(), &complete_session)
                .await
                .unwrap(),
            MutationRemoteOutcome::Committed { .. }
        ));
        drop(backend);
        drop(store);

        let restarted =
            Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let generation_two = SessionGeneration::new(2).unwrap();
        restarted
            .activate_session(&scope, generation_two)
            .await
            .unwrap();
        let restarted_backend =
            MemoryRemoteMutationBackend::new(restarted.clone(), Some(&directory)).unwrap();
        assert_eq!(
            ContentUploadRunner::new(8)
                .unwrap()
                .resume(
                    &operation_id,
                    generation_two,
                    &*restarted,
                    &restarted_backend,
                    &*restarted,
                )
                .await
                .unwrap(),
            ContentUploadRunOutcome::Completed
        );
        let completed = restarted
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            completed.remote_outcome(),
            Some(MutationRemoteOutcome::AlreadyCommitted { .. })
                | Some(MutationRemoteOutcome::Committed { .. })
        ));
        assert_eq!(
            committed_upload_bytes(&restarted_backend, completed.intent()),
            Some(Bytes::from_static(b"restart-upload-bytes"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn legacy_dirty_state_reconstructs_upload_intent_and_immutable_bytes() {
        let directory = test_directory("legacy-upload");
        fs::create_dir_all(&directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let commit = write_created(&store, &acceptance, b"legacy-upload-bytes").await;
        let writeback = directory.join("writeback.json");
        let mut document: DurableWritebackDocument =
            serde_json::from_slice(&fs::read(&writeback).unwrap()).unwrap();
        document.uploads.clear();
        document.immutable_snapshots.clear();
        fs::write(&writeback, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        drop(store);

        let recovered = MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap();
        let records = recovered
            .recoverable_content_uploads_page(&scope, None, RECOVERY_BATCH_LIMIT)
            .await
            .unwrap()
            .into_items();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state(), ContentUploadState::IntentPersisted);
        assert_eq!(
            recovered
                .read_snapshot(commit.snapshot(), 0, commit.snapshot().size())
                .await
                .unwrap(),
            Bytes::from_static(b"legacy-upload-bytes")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn newer_mount_fences_an_old_upload_executor_without_losing_its_record() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation_one = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation_one).await;
        let commit = write_created(&store, &acceptance, b"fenced-upload").await;
        let operation_id = upload_operation(commit.snapshot());
        let generation_two = SessionGeneration::new(2).unwrap();
        store
            .activate_session(&scope, generation_two)
            .await
            .unwrap();
        let backend = MemoryRemoteMutationBackend::new(store.clone(), None).unwrap();
        assert_eq!(
            ContentUploadRunner::new(8)
                .unwrap()
                .resume(&operation_id, generation_one, &*store, &backend, &*store,)
                .await
                .unwrap(),
            ContentUploadRunOutcome::Fenced
        );
        assert_eq!(
            store
                .load_content_upload(&operation_id)
                .await
                .unwrap()
                .unwrap()
                .state(),
            ContentUploadState::IntentPersisted
        );
    }

    #[tokio::test]
    async fn failed_local_writeback_persist_does_not_publish_bytes_or_upload_intent() {
        let directory = test_directory("writeback-persist-failure");
        fs::create_dir_all(&directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        fs::remove_file(directory.join("writeback.json")).unwrap();
        fs::remove_dir(&directory).unwrap();
        fs::write(&directory, b"file").unwrap();
        let error = store
            .write(
                acceptance.session().id(),
                0,
                Bytes::from_static(b"must-not-publish"),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), CloudFilesStoreErrorKind::PersistenceFailure);
        assert!(
            store
                .read(acceptance.session().id(), 0, 64)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .recoverable_content_uploads_page(&scope, None, RECOVERY_BATCH_LIMIT)
                .await
                .unwrap()
                .items()
                .is_empty()
        );
        fs::remove_file(directory).unwrap();
    }

    #[tokio::test]
    async fn failed_remote_upload_commit_persist_does_not_publish_committed_content() {
        let directory = test_directory("upload-persist-failure");
        let remote_directory = directory.join("remote-state");
        fs::create_dir_all(&remote_directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let commit = write_created(&store, &acceptance, b"remote-persist-failure").await;
        let operation_id = upload_operation(commit.snapshot());
        let record = store
            .load_content_upload(&operation_id)
            .await
            .unwrap()
            .unwrap();
        let backend =
            MemoryRemoteMutationBackend::new(store.clone(), Some(&remote_directory)).unwrap();
        let session = backend.start_upload(record.intent()).await.unwrap();
        let bytes = store
            .read_snapshot(
                record.intent().snapshot(),
                0,
                record.intent().snapshot().size(),
            )
            .await
            .unwrap();
        let chunk = ContentUploadChunk::new(
            session.id().clone(),
            record.intent().snapshot().generation(),
            0,
            bytes,
            record.intent().snapshot().size(),
        )
        .unwrap();
        let acknowledgement = backend.upload_chunk(record.intent(), &chunk).await.unwrap();
        let complete_session = ContentUploadSession::new(
            acknowledgement.session_id().clone(),
            acknowledgement.accepted_offset(),
        );
        fs::remove_file(remote_directory.join("remote.json")).unwrap();
        fs::remove_dir(&remote_directory).unwrap();
        fs::write(&remote_directory, b"file").unwrap();
        let error = backend
            .commit_upload(record.intent(), &complete_session)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), CloudBackendErrorKind::TemporarilyUnavailable);
        assert_eq!(committed_upload_bytes(&backend, record.intent()), None);
        fs::remove_file(remote_directory).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn remote_commit_before_outcome_marker_recovers_after_restart() {
        let directory = test_directory("remote-restart");
        fs::create_dir_all(&directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation_one = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let acceptance = accepted_create(&store, &scope, generation_one).await;
        let backend = MemoryRemoteMutationBackend::new(store.clone(), Some(&directory)).unwrap();
        let operation_id = acceptance
            .created()
            .mutation_intent()
            .operation_id()
            .clone();
        store
            .begin_remote_apply(&operation_id, generation_one)
            .await
            .unwrap();
        let outcome = backend
            .apply_mutation(acceptance.created().mutation_intent())
            .await
            .unwrap();
        assert!(matches!(outcome, MutationRemoteOutcome::Committed { .. }));
        drop(backend);
        drop(store);

        let restarted =
            Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let generation_two = SessionGeneration::new(2).unwrap();
        restarted
            .activate_session(&scope, generation_two)
            .await
            .unwrap();
        let restarted_backend =
            MemoryRemoteMutationBackend::new(restarted.clone(), Some(&directory)).unwrap();
        let result = MutationRunner
            .resume(
                &operation_id,
                generation_two,
                &*restarted,
                &restarted_backend,
            )
            .await
            .unwrap();
        assert_eq!(result, MutationRunOutcome::Completed);
        let record = restarted
            .load_mutation(&operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.state(), MutationState::Completed);
        assert!(matches!(
            record.remote_outcome(),
            Some(MutationRemoteOutcome::AlreadyCommitted { .. })
                | Some(MutationRemoteOutcome::Committed { .. })
        ));
        assert!(directory.join("remote.json").is_file());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn legacy_writeback_without_mutation_vector_is_recovered() {
        let directory = test_directory("legacy-mutation");
        fs::create_dir_all(&directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
        let _acceptance = accepted_create(&store, &scope, generation).await;
        let writeback = directory.join("writeback.json");
        let mut document: DurableWritebackDocument =
            serde_json::from_slice(&fs::read(&writeback).unwrap()).unwrap();
        document.mutations.clear();
        fs::write(&writeback, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

        let recovered = MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap();
        let records = recovered
            .recoverable_mutations_page(&scope, None, RECOVERY_BATCH_LIMIT)
            .await
            .unwrap()
            .into_items();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state(), MutationState::IntentPersisted);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn failed_remote_persist_does_not_publish_an_in_memory_commit() {
        let directory = test_directory("remote-persist-failure");
        fs::create_dir_all(&directory).unwrap();
        let remote_state_directory = directory.join("remote-state");
        fs::create_dir_all(&remote_state_directory).unwrap();
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = Arc::new(MemoryWritebackStore::new(scope.clone(), None).unwrap());
        let acceptance = accepted_create(&store, &scope, generation).await;
        let backend =
            MemoryRemoteMutationBackend::new(store, Some(&remote_state_directory)).unwrap();
        fs::remove_dir(&remote_state_directory).unwrap();
        fs::write(&remote_state_directory, b"file").unwrap();
        let intent = acceptance.created().mutation_intent();
        let error = backend.apply_mutation(intent).await.unwrap_err();
        assert_eq!(error.kind(), CloudBackendErrorKind::TemporarilyUnavailable);
        assert_eq!(
            backend.reconcile_mutation(intent).await.unwrap(),
            MutationRemoteOutcome::RemoteOutcomeUnknown
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn closing_session_rejects_a_late_native_create() {
        let (cloud, _) = MemoryCloud::fixture().unwrap();
        let scope = cloud.scope.clone();
        let generation = SessionGeneration::new(1).unwrap();
        let store = MemoryWritebackStore::new(scope.clone(), None).unwrap();
        store.activate_session(&scope, generation).await.unwrap();
        store
            .transition_session(&scope, generation, SessionState::Closing)
            .await
            .unwrap();
        let request = LinuxCreateFileRequest::new(
            key(&scope, "root").unwrap(),
            "late.txt",
            0o100644,
            0,
            aster_forge_cloud_files_linux::LinuxFileAccess::ReadWrite,
            generation,
        )
        .unwrap();
        let error = store.create_file(&request).await.unwrap_err();
        assert_eq!(error.kind(), LinuxNamespaceMutationStoreErrorKind::Fenced);
        assert!(
            store
                .recoverable_mutations_page(&scope, None, RECOVERY_BATCH_LIMIT)
                .await
                .unwrap()
                .items()
                .is_empty()
        );
    }
}
