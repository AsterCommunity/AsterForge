#[async_trait]
impl MutationJournalStore for MemoryWritebackStore {
    async fn activate_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.scope_matches(scope)?;
        let mut state = lock(&self.state);
        let mut next = state.clone();
        match next.active_generation {
            Some(active) if generation < active => return Ok(StoreWriteStatus::Fenced),
            Some(active) if generation == active => {
                if next.active_session_state == Some(SessionState::Accepting) {
                    return Ok(StoreWriteStatus::AlreadyApplied);
                }
                return Err(store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic session generation cannot reopen after closing",
                ));
            }
            Some(_) | None => {
                next.active_generation = Some(generation);
                next.active_session_state = Some(SessionState::Accepting);
            }
        }
        self.persist(&next)?;
        *state = next;
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_session(
        &self,
        scope: &CloudScope,
    ) -> StoreResult<Option<(SessionGeneration, SessionState)>> {
        self.scope_matches(scope)?;
        let state = lock(&self.state);
        Ok(state
            .active_generation
            .zip(state.active_session_state)
            .or_else(|| {
                state
                    .active_generation
                    .map(|generation| (generation, SessionState::Accepting))
            }))
    }

    async fn transition_session(
        &self,
        scope: &CloudScope,
        generation: SessionGeneration,
        next_state: SessionState,
    ) -> StoreResult<StoreWriteStatus> {
        self.scope_matches(scope)?;
        let mut state = lock(&self.state);
        let active = state.active_generation.ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic session was not activated",
            )
        })?;
        if generation < active {
            return Ok(StoreWriteStatus::Fenced);
        }
        if generation > active {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "synthetic session generation is not active",
            ));
        }
        let current = state
            .active_session_state
            .unwrap_or(SessionState::Accepting);
        if current == next_state {
            return Ok(StoreWriteStatus::AlreadyApplied);
        }
        if !current.can_transition_to(next_state) {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "invalid synthetic session lifecycle transition",
            ));
        }
        let mut next = state.clone();
        next.active_session_state = Some(next_state);
        self.persist(&next)?;
        *state = next;
        Ok(StoreWriteStatus::Applied)
    }

    async fn persist_mutation_intent(
        &self,
        intent: MutationIntent,
    ) -> StoreResult<StoreWriteStatus> {
        self.scope_matches(intent.desired().scope())?;
        let record = MutationRecord::persist(intent).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let operation_id = record.intent().operation_id().clone();
        let idempotency_key = record.intent().idempotency_key().clone();
        let generation = record.intent().session_generation();
        let mut state = lock(&self.state);
        if !self.active_for_generation(&state, record.intent().desired().scope(), generation)?
            || state.active_session_state != Some(SessionState::Accepting)
        {
            return Ok(StoreWriteStatus::Fenced);
        }
        if let Some(existing) = state.mutations.get(&operation_id) {
            return if existing == &record {
                Ok(StoreWriteStatus::AlreadyApplied)
            } else {
                Err(store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "synthetic operation identity already identifies another mutation",
                ))
            };
        }
        if state
            .idempotency_keys
            .get(&idempotency_key)
            .is_some_and(|existing| existing != &operation_id)
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "synthetic idempotency key already identifies another operation",
            ));
        }
        let mut next = state.clone();
        next.idempotency_keys
            .insert(idempotency_key, operation_id.clone());
        next.mutations.insert(operation_id, record);
        self.persist(&next)?;
        *state = next;
        self.mutation_notify.notify_one();
        Ok(StoreWriteStatus::Applied)
    }

    async fn load_mutation(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<Option<MutationRecord>> {
        Ok(lock(&self.state).mutations.get(operation_id).cloned())
    }

    async fn recoverable_mutations_page(
        &self,
        scope: &CloudScope,
        after_operation_id: Option<&str>,
        limit: usize,
    ) -> StoreResult<RecoveryPage<MutationRecord>> {
        self.scope_matches(scope)?;
        if limit == 0 {
            return Err(store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "synthetic mutation recovery page limit must be non-zero",
            ));
        }
        let state = lock(&self.state);
        let mut selected = Vec::with_capacity(limit.saturating_add(1).min(state.mutations.len()));
        for record in state.mutations.values().filter(|record| {
            record.intent().desired().scope() == scope
                && record.state() != MutationState::Completed
                && after_operation_id
                    .is_none_or(|after| record.intent().operation_id().as_str() > after)
        }) {
            let operation_id = record.intent().operation_id().as_str();
            let index = selected.partition_point(|selected: &&MutationRecord| {
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

    async fn begin_remote_apply(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_mutation(operation_id, generation, MutationRecord::begin_remote_apply)
    }

    async fn record_remote_outcome(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
        outcome: MutationRemoteOutcome,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_mutation(operation_id, generation, |record| {
            record.record_remote_outcome(outcome)
        })
    }

    async fn mark_platform_reconciled(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = lock(&self.state);
        let scope = state
            .mutations
            .get(operation_id)
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic mutation was not found",
                )
            })?
            .intent()
            .desired()
            .scope()
            .clone();
        if !self.active_for_generation(&state, &scope, generation)? {
            return Ok(StoreWriteStatus::Fenced);
        }
        let mut next = state.clone();
        let desired = next
            .mutations
            .get(operation_id)
            .map(|record| record.intent().desired().clone())
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic mutation disappeared",
                )
            })?;
        let outcome = next
            .mutations
            .get(operation_id)
            .and_then(|record| record.remote_outcome().cloned());
        if let Some(
            MutationRemoteOutcome::Committed { item: Some(item) }
            | MutationRemoteOutcome::AlreadyCommitted { item: Some(item) },
        ) = outcome
        {
            match desired {
                DesiredMutation::Create {
                    kind: CloudItemKind::File,
                    ..
                } => {
                    let key = Self::created_operation_key(&next, operation_id)?;
                    let created = next.created.get(&key).cloned().ok_or_else(|| {
                        store_error(
                            CloudFilesStoreErrorKind::NotFound,
                            "synthetic platform reconciliation lost its local created file",
                        )
                    })?;
                    if created.item().key() != item.key() || item.kind() != CloudItemKind::File {
                        return Err(store_error(
                            CloudFilesStoreErrorKind::InvalidTransition,
                            "synthetic remote create changed the preallocated file identity",
                        ));
                    }
                    let (_, inode_record, intent) = created.into_parts();
                    next.created
                        .insert(key, LinuxCreatedFile::new(item, inode_record, intent));
                }
                DesiredMutation::Create {
                    kind: CloudItemKind::Directory,
                    ..
                }
                | DesiredMutation::ModifyMetadata { .. } => {
                    let current = next.namespace_items.get(item.key()).cloned();
                    if let Some(current) = current.filter(|current| {
                        current.mutation_intent().operation_id() == operation_id
                    }) {
                        if current.item().kind() != item.kind() {
                            return Err(store_error(
                                CloudFilesStoreErrorKind::InvalidTransition,
                                "synthetic remote namespace mutation changed item kind",
                            ));
                        }
                        let (_, inode_record, intent) = current.into_parts();
                        next.namespace_items.insert(
                            item.key().clone(),
                            LinuxNamespaceItem::new(item, inode_record, intent),
                        );
                    }
                }
                DesiredMutation::Delete { .. } | DesiredMutation::ModifyContent { .. } => {
                    return Err(store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic remote mutation returned unexpected item metadata",
                    ));
                }
            }
        }
        let record = next.mutations.get_mut(operation_id).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic mutation disappeared",
            )
        })?;
        let status = record.mark_platform_reconciled().map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let write_status = match status {
            MutationRecordTransition::Applied => StoreWriteStatus::Applied,
            MutationRecordTransition::AlreadyApplied => StoreWriteStatus::AlreadyApplied,
        };
        if write_status == StoreWriteStatus::Applied {
            self.persist(&next)?;
            *state = next;
        }
        Ok(write_status)
    }

    async fn complete_mutation(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
    ) -> StoreResult<StoreWriteStatus> {
        self.transition_mutation(operation_id, generation, MutationRecord::complete)
    }
}
