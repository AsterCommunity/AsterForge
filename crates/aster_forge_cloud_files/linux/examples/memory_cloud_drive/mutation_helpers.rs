impl MemoryWritebackStore {
    fn scope_matches(&self, scope: &CloudScope) -> StoreResult<()> {
        if &self.scope == scope {
            Ok(())
        } else {
            Err(store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic mutation scope was not configured for this store",
            ))
        }
    }

    fn active_for_generation(
        &self,
        state: &MemoryWritebackState,
        scope: &CloudScope,
        generation: SessionGeneration,
    ) -> StoreResult<bool> {
        self.scope_matches(scope)?;
        let Some(active) = state.active_generation else {
            return Err(store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic mutation session was not activated",
            ));
        };
        Ok(active == generation && state.active_session_state != Some(SessionState::Closed))
    }

    fn transition_mutation(
        &self,
        operation_id: &OperationId,
        generation: SessionGeneration,
        transition: impl FnOnce(
            &mut MutationRecord,
        )
            -> aster_forge_cloud_files_core::Result<MutationRecordTransition>,
    ) -> StoreResult<StoreWriteStatus> {
        let mut state = lock(&self.state);
        let scope = state
            .mutations
            .get(operation_id)
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic mutation operation was not found",
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
        let record = next.mutations.get_mut(operation_id).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "synthetic mutation operation disappeared",
            )
        })?;
        let status = transition(record).map_err(|error| {
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

    fn created_operation_key(
        state: &MemoryWritebackState,
        operation_id: &OperationId,
    ) -> StoreResult<CloudItemKey> {
        state
            .created
            .iter()
            .find_map(|(key, created)| {
                (created.mutation_intent().operation_id() == operation_id).then(|| key.clone())
            })
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic mutation had no local created item",
                )
            })
    }

    fn namespace_item_for_operation(
        &self,
        operation_id: &OperationId,
    ) -> StoreResult<(CloudItem, u64)> {
        let state = lock(&self.state);
        if let Some(created) = state
            .created
            .values()
            .find(|created| created.mutation_intent().operation_id() == operation_id)
        {
            let size = state
                .files
                .get(created.item().key())
                .map_or(0, |file| file.bytes.len());
            return Ok((
                created.item().clone(),
                u64::try_from(size).map_err(|_| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic staged size exceeded u64",
                    )
                })?,
            ));
        }
        state
            .namespace_items
            .values()
            .find(|item| item.mutation_intent().operation_id() == operation_id)
            .map(|item| {
                (
                    item.item().clone(),
                    item.item().content().map_or(0, CloudContentMetadata::size),
                )
            })
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic mutation had no local namespace item",
                )
            })
    }
}
