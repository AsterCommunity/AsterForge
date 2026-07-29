impl MemoryWritebackStore {
    fn transition_upload(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
        transition: impl FnOnce(
            &mut ContentUploadRecord,
        )
            -> aster_forge_cloud_files_core::Result<ContentUploadRecordTransition>,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = lock(&self.state);
        let scope = state
            .uploads
            .get(operation_id)
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic content upload was not found",
                )
            })?
            .intent()
            .base_cache_key()
            .item_key()
            .scope()
            .clone();
        if !self.active_for_generation(&state, &scope, execution_generation)? {
            return Ok(StoreWriteStatus::Fenced);
        }
        let mut next = state.clone();
        let record = next.uploads.get_mut(operation_id).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic content upload disappeared",
            )
        })?;
        let status = transition(record).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let write_status = match status {
            ContentUploadRecordTransition::Applied => StoreWriteStatus::Applied,
            ContentUploadRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
            ContentUploadRecordTransition::Fenced => StoreWriteStatus::Fenced,
        };
        if write_status == StoreWriteStatus::Applied {
            Self::compact_terminal_records(&mut next);
            self.persist(&next)?;
            *state = next;
        }
        Ok(write_status)
    }
}

#[async_trait]
impl ContentUploadStore for MemoryWritebackStore {
    async fn persist_content_upload_intent(
        &self,
        intent: ContentUploadIntent,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.scope_matches(intent.base_cache_key().item_key().scope())?;
        let mut state = lock(&self.state);
        if !self.active_for_generation(
            &state,
            intent.base_cache_key().item_key().scope(),
            execution_generation,
        )? || state.active_session_state != Some(SessionState::Accepting)
        {
            return Ok(StoreWriteStatus::Fenced);
        }
        let file = state
            .files
            .get(intent.snapshot().item_key())
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic upload staging was not found",
                )
            })?;
        if file.snapshot.as_ref() != Some(intent.snapshot()) {
            return Ok(StoreWriteStatus::Fenced);
        }
        let immutable = state
            .immutable_snapshots
            .get(&(
                intent.snapshot().item_key().clone(),
                intent.snapshot().generation(),
            ))
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic immutable upload bytes were not found",
                )
            })?;
        if immutable.reference != *intent.snapshot().reference()
            || u64::try_from(immutable.bytes.len()).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic immutable upload size exceeded u64",
                )
            })? != intent.snapshot().size()
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic immutable upload metadata drifted",
            ));
        }
        let operation_id = intent.operation_id().clone();
        let idempotency_key = intent.idempotency_key().clone();
        let record = ContentUploadRecord::persist(intent);
        if let Some(existing) = state.uploads.get(&operation_id) {
            return if existing == &record {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "synthetic upload operation identifies another intent",
                ))
            };
        }
        if state
            .upload_idempotency_keys
            .get(&idempotency_key)
            .is_some_and(|existing| existing != &operation_id)
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "synthetic upload idempotency key identifies another operation",
            ));
        }
        let mut next = state.clone();
        next.upload_idempotency_keys
            .insert(idempotency_key, operation_id.clone());
        next.uploads.insert(operation_id, record);
        self.persist(&next)?;
        *state = next;
        self.upload_notify.notify_one();
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_content_upload(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<ContentUploadRecord>> {
        Ok(lock(&self.state).uploads.get(operation_id).cloned())
    }

    async fn recoverable_content_uploads_page(
        &self,
        scope: &CloudScope,
        after_operation_id: Option<&str>,
        limit: usize,
    ) -> StoreResult<RecoveryPage<ContentUploadRecord>> {
        self.scope_matches(scope)?;
        if limit == 0 {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload recovery page limit must be non-zero",
            ));
        }
        let state = lock(&self.state);
        let mut selected = Vec::with_capacity(limit.saturating_add(1).min(state.uploads.len()));
        for record in state.uploads.values().filter(|record| {
            record.intent().base_cache_key().item_key().scope() == scope
                && record.state() != ContentUploadState::Completed
                && after_operation_id
                    .is_none_or(|after| record.intent().operation_id().as_str() > after)
        }) {
            let operation_id = record.intent().operation_id().as_str();
            let index = selected.partition_point(|selected: &&ContentUploadRecord| {
                selected.intent().operation_id().as_str() < operation_id
            });
            selected.insert(index, record);
            if selected.len() > limit.saturating_add(1) {
                selected.pop();
            }
        }
        let has_more = selected.len() > limit;
        selected.truncate(limit);
        let records = selected.into_iter().cloned().collect();
        Ok(RecoveryPage::new(records, has_more))
    }

    async fn record_content_upload_session(
        &self,
        operation_id: &OperationId,
        session: ContentUploadSession,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_upload(operation_id, execution_generation, |record| {
            record.record_session(session)
        })
    }

    async fn begin_content_upload_remote_commit(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_upload(
            operation_id,
            execution_generation,
            ContentUploadRecord::begin_remote_commit,
        )
    }

    async fn record_content_upload_remote_outcome(
        &self,
        operation_id: &OperationId,
        outcome: MutationRemoteOutcome,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_upload(operation_id, execution_generation, |record| {
            record.record_remote_outcome(outcome)
        })
    }

    async fn reconcile_content_upload_metadata(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = lock(&self.state);
        let record = state.uploads.get(operation_id).cloned().ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic content upload was not found",
            )
        })?;
        let scope = record.intent().base_cache_key().item_key().scope();
        if !self.active_for_generation(&state, scope, execution_generation)? {
            return Ok(StoreWriteStatus::Fenced);
        }
        let mut next = state.clone();
        let outcome = record.remote_outcome().cloned().ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload metadata reconciliation omitted its outcome",
            )
        })?;
        if let MutationRemoteOutcome::Committed { item: Some(item) }
        | MutationRemoteOutcome::AlreadyCommitted { item: Some(item) } = outcome
        {
            let content = item.content().ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic uploaded item omitted content metadata",
                )
            })?;
            let key = record.intent().snapshot().item_key();
            let file = next.files.get_mut(key).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic uploaded staging disappeared",
                )
            })?;
            file.base_revision = content.revision().clone();
            if file.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.generation() == record.intent().snapshot().generation()
            }) {
                file.snapshot = None;
            }
            if let Some(created) = next.created.get(key).cloned() {
                let created_item = created.item();
                let parent_id = created_item.parent_id().cloned().ok_or_else(|| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic created upload baseline omitted its parent",
                    )
                })?;
                let merged = CloudItem::file(
                    key.clone(),
                    parent_id,
                    created_item.name(),
                    item.metadata_revision().clone(),
                    content.clone(),
                )
                .map_err(|error| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        format!("merge synthetic upload metadata: {error}"),
                    )
                })?;
                let (_, inode_record, intent) = created.into_parts();
                next.created.insert(
                    key.clone(),
                    LinuxCreatedFile::new(merged, inode_record, intent),
                );
            }
        }
        let upload = next.uploads.get_mut(operation_id).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic content upload disappeared",
            )
        })?;
        let status = upload.mark_metadata_reconciled().map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let write_status = match status {
            ContentUploadRecordTransition::Applied => StoreWriteStatus::Applied,
            ContentUploadRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
            ContentUploadRecordTransition::Fenced => StoreWriteStatus::Fenced,
        };
        if write_status == StoreWriteStatus::Applied {
            self.persist(&next)?;
            *state = next;
        }
        Ok(write_status)
    }

    async fn complete_content_upload(
        &self,
        operation_id: &OperationId,
        execution_generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_upload(
            operation_id,
            execution_generation,
            ContentUploadRecord::complete,
        )
    }
}

#[async_trait]
impl LocalContentSnapshotReader for MemoryWritebackStore {
    async fn read_snapshot(
        &self,
        snapshot: &LocalContentSnapshot,
        offset: u64,
        length: u64,
    ) -> StoreResult<Bytes> {
        if length == 0 {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload reader requires a non-empty range",
            ));
        }
        let end = offset.checked_add(length).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload source range overflowed u64",
            )
        })?;
        if end > snapshot.size() {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload source range exceeded its snapshot",
            ));
        }
        let state = lock(&self.state);
        let immutable = state
            .immutable_snapshots
            .get(&(snapshot.item_key().clone(), snapshot.generation()))
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic immutable upload source was not found",
                )
            })?;
        if immutable.reference != *snapshot.reference()
            || u64::try_from(immutable.bytes.len()).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic immutable source size exceeded u64",
                )
            })? != snapshot.size()
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic immutable upload source metadata drifted",
            ));
        }
        let start = usize::try_from(offset).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload source offset exceeded usize",
            )
        })?;
        let end = usize::try_from(end).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic upload source end exceeded usize",
            )
        })?;
        Ok(Bytes::copy_from_slice(&immutable.bytes[start..end]))
    }
}
