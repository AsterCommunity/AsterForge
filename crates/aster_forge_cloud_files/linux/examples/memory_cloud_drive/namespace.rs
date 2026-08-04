impl MemoryWritebackStore {
    fn namespace_accepting(
        state: &MemoryWritebackState,
        generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<()> {
        if state.active_generation != Some(generation)
            || state.active_session_state != Some(SessionState::Accepting)
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "synthetic namespace request used a fenced mount generation",
            ));
        }
        Ok(())
    }

    fn fixture_namespace_item(
        key: &CloudItemKey,
    ) -> LinuxNamespaceStoreResult<Option<(CloudItem, LinuxInodeRecord)>> {
        let (cloud, records) = MemoryCloud::fixture().map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                format!("load synthetic fixture namespace: {error}"),
            )
        })?;
        let Some(item) = cloud.items.get(key).cloned() else {
            return Ok(None);
        };
        let record = records
            .into_iter()
            .find(|record| record.key() == key)
            .ok_or_else(|| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic fixture item omitted its inode record",
                )
            })?;
        Ok(Some((item, record)))
    }

    fn current_namespace_item(
        state: &MemoryWritebackState,
        key: &CloudItemKey,
    ) -> LinuxNamespaceStoreResult<Option<(CloudItem, LinuxInodeRecord)>> {
        if let Some(item) = state.namespace_items.get(key) {
            return Ok(Some((item.item().clone(), item.inode_record().clone())));
        }
        if let Some(created) = state.created.get(key) {
            return Ok(Some((
                created.item().clone(),
                created.inode_record().clone(),
            )));
        }
        Self::fixture_namespace_item(key)
    }

    fn current_name_key(
        state: &MemoryWritebackState,
        parent_id: &CloudItemId,
        name: &str,
    ) -> LinuxNamespaceStoreResult<Option<CloudItemKey>> {
        let name_key = (parent_id.clone(), name.to_owned());
        if state.tombstones.contains_key(&name_key)
            && !state.namespace_names.contains_key(&name_key)
            && !state.created_names.contains_key(&name_key)
        {
            return Ok(None);
        }
        if let Some(key) = state.namespace_names.get(&name_key) {
            return Ok(Some(key.clone()));
        }
        if let Some(key) = state.created_names.get(&name_key) {
            return Ok(Some(key.clone()));
        }
        let (cloud, _) = MemoryCloud::fixture().map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                format!("load synthetic fixture namespace: {error}"),
            )
        })?;
        Ok(cloud
            .items
            .values()
            .find(|item| item.parent_id() == Some(parent_id) && item.name() == name)
            .map(|item| item.key().clone()))
    }

    fn directory_is_empty(
        state: &MemoryWritebackState,
        key: &CloudItemKey,
    ) -> LinuxNamespaceStoreResult<bool> {
        let (cloud, _) = MemoryCloud::fixture().map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                format!("load synthetic fixture namespace: {error}"),
            )
        })?;
        let fixture_has_child = cloud.items.values().any(|item| {
            item.parent_id() == Some(key.item_id())
                && !state
                    .tombstones
                    .contains_key(&(key.item_id().clone(), item.name().to_owned()))
        });
        let local_has_child = state
            .created_names
            .keys()
            .chain(state.namespace_names.keys())
            .any(|(parent_id, _)| parent_id == key.item_id());
        Ok(!fixture_has_child && !local_has_child)
    }

    fn next_namespace_identity(state: &mut MemoryWritebackState) -> LinuxNamespaceStoreResult<u64> {
        state.next_item = state.next_item.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic namespace operation identity exhausted",
            )
        })?;
        Ok(state.next_item)
    }

    fn namespace_intent(
        sequence: u64,
        generation: SessionGeneration,
        desired: DesiredMutation,
        preconditions: MutationPreconditions,
    ) -> LinuxNamespaceStoreResult<MutationIntent> {
        Ok(MutationIntent::new(
            OperationId::new(format!("namespace-op-{sequence:020}")).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic namespace operation id was invalid",
                )
            })?,
            IdempotencyKey::new(format!("namespace-key-{sequence:020}")).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic namespace idempotency key was invalid",
                )
            })?,
            MutationOrigin::PlatformCommand,
            generation,
            desired,
            preconditions,
        ))
    }

    fn insert_namespace_mutation(
        state: &mut MemoryWritebackState,
        intent: MutationIntent,
    ) -> LinuxNamespaceStoreResult<()> {
        let record = MutationRecord::persist(intent).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                format!("synthetic namespace mutation was invalid: {error}"),
            )
        })?;
        let operation_id = record.intent().operation_id().clone();
        if state.mutations.contains_key(&operation_id)
            || state
                .idempotency_keys
                .insert(
                    record.intent().idempotency_key().clone(),
                    operation_id.clone(),
                )
                .is_some()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic namespace mutation identity already existed",
            ));
        }
        state.mutations.insert(operation_id, record);
        Ok(())
    }
}

#[async_trait]
#[expect(
    clippy::too_many_lines,
    reason = "the synthetic namespace adapter keeps each durable acceptance contract visible in one product-side implementation"
)]
impl LinuxNamespaceMutationStore for MemoryWritebackStore {
    async fn activate_namespace(
        &self,
        scope: &CloudScope,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<Vec<LinuxCreatedFile>> {
        let mut state = lock(&self.state);
        if state
            .active_generation
            .is_some_and(|current| session_generation < current)
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "synthetic namespace mount generation regressed",
            ));
        }
        let mut next = state.clone();
        next.active_generation = Some(session_generation);
        self.persist(&next).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })?;
        *state = next;
        Ok(state
            .created
            .values()
            .filter(|created| created.item().key().scope() == scope)
            .cloned()
            .collect())
    }

    async fn create_file(
        &self,
        request: &LinuxCreateFileRequest,
    ) -> LinuxNamespaceStoreResult<LinuxCreateFileAcceptance> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(request.session_generation())
            || state.active_session_state != Some(SessionState::Accepting)
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "synthetic create used a fenced mount generation",
            ));
        }
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        if Self::current_name_key(&state, request.parent_key().item_id(), request.name())?.is_some()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
                "synthetic parent/name already exists",
            ));
        }

        let mut next = state.clone();
        next.next_item = next.next_item.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic item identity exhausted",
            )
        })?;
        next.next_inode = next.next_inode.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic inode identity exhausted",
            )
        })?;
        next.next_session = next.next_session.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic write session exhausted",
            )
        })?;
        let key = CloudItemKey::new(
            request.parent_key().scope().clone(),
            CloudItemId::new(format!("local-created-{}", next.next_item)).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic item identity was invalid",
                )
            })?,
        );
        let item = CloudItem::file(
            key.clone(),
            request.parent_key().item_id().clone(),
            request.name(),
            MetadataRevision::from_slice(format!("local-create-m{}", next.next_item).as_bytes())
                .map_err(|_| {
                    namespace_store_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "synthetic metadata revision was invalid",
                    )
                })?,
            CloudContentMetadata::new(
                ContentRevision::from_slice(format!("local-create-c{}", next.next_item).as_bytes())
                    .map_err(|_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic content revision was invalid",
                        )
                    })?,
                None,
                0,
            ),
        )
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic created item was invalid",
            )
        })?;
        let base_revision = item
            .content()
            .map(|content| content.revision().clone())
            .ok_or_else(|| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic created file omitted its content revision",
                )
            })?;
        let record = LinuxInodeRecord::new(
            key.clone(),
            LinuxInode::new(next.next_inode).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic inode was invalid",
                )
            })?,
            LinuxInodeGeneration::new(next.next_item).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic inode generation was invalid",
                )
            })?,
        );
        let intent = MutationIntent::new(
            OperationId::new(format!("local-create-op-{}", next.next_item)).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic operation identity was invalid",
                )
            })?,
            IdempotencyKey::new(format!("local-create-key-{}", next.next_item)).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic idempotency key was invalid",
                )
            })?,
            MutationOrigin::PlatformCommand,
            request.session_generation(),
            DesiredMutation::create(
                request.parent_key().scope().clone(),
                request.parent_key().item_id().clone(),
                request.name(),
                CloudItemKind::File,
            )
            .map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic create mutation was invalid",
                )
            })?,
            MutationPreconditions::default(),
        );
        let mutation = MutationRecord::persist(intent.clone()).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                format!("synthetic create mutation was not persistable: {error}"),
            )
        })?;
        if next
            .mutations
            .contains_key(mutation.intent().operation_id())
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic create operation identity already exists",
            ));
        }
        if next
            .idempotency_keys
            .insert(
                mutation.intent().idempotency_key().clone(),
                mutation.intent().operation_id().clone(),
            )
            .is_some()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic create idempotency key already exists",
            ));
        }
        let created = LinuxCreatedFile::new(item, record, intent);
        let session_id = LinuxWriteSessionId::new(format!("memory-session-{}", next.next_session))
            .map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic write session identity was invalid",
                )
            })?;
        next.files.insert(
            key.clone(),
            StagedFile {
                bytes: Arc::from([]),
                snapshot: None,
                base_revision,
            },
        );
        next.sessions.insert(
            session_id.clone(),
            WriteSessionBinding {
                key: key.clone(),
                generation: request.session_generation(),
            },
        );
        next.created_names.insert(name_key, key.clone());
        next.created.insert(key.clone(), created.clone());
        next.mutations
            .insert(mutation.intent().operation_id().clone(), mutation);
        self.persist(&next).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })?;
        *state = next;
        drop(state);
        self.mutation_notify.notify_one();
        Ok(LinuxCreateFileAcceptance::new(
            created,
            LinuxWriteSession::new(session_id, key, 0, request.session_generation()),
        ))
    }

    async fn open_created_file(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<LinuxWriteSession> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(session_generation) {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "synthetic created open used a fenced mount generation",
            ));
        }
        if !state.created.contains_key(key) || !state.files.contains_key(key) {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::NotFound,
                "synthetic created item staging was missing",
            ));
        }
        state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic write session exhausted",
            )
        })?;
        let id = LinuxWriteSessionId::new(format!("memory-session-{}", state.next_session))
            .map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic write session identity was invalid",
                )
            })?;
        let size = u64::try_from(
            state
                .files
                .get(key)
                .map(|file| file.bytes.len())
                .unwrap_or_default(),
        )
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic staging size exceeded u64",
            )
        })?;
        state.sessions.insert(
            id.clone(),
            WriteSessionBinding {
                key: key.clone(),
                generation: session_generation,
            },
        );
        Ok(LinuxWriteSession::new(
            id,
            key.clone(),
            size,
            session_generation,
        ))
    }

    async fn activate_namespace_overlay(
        &self,
        scope: &CloudScope,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceOverlay> {
        let state = lock(&self.state);
        if state.active_generation != Some(session_generation) {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "synthetic namespace overlay used a fenced mount generation",
            ));
        }
        Ok(LinuxNamespaceOverlay::new(
            state
                .namespace_items
                .values()
                .filter(|item| item.item().key().scope() == scope)
                .cloned()
                .collect(),
            state
                .tombstones
                .values()
                .filter(|tombstone| tombstone.key().scope() == scope)
                .cloned()
                .collect(),
        ))
    }

    async fn create_directory(
        &self,
        request: &LinuxCreateDirectoryRequest,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceItem> {
        let mut state = lock(&self.state);
        Self::namespace_accepting(&state, request.session_generation())?;
        if Self::current_name_key(&state, request.parent_key().item_id(), request.name())?.is_some()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
                "synthetic directory parent/name already exists",
            ));
        }
        let mut next = state.clone();
        let sequence = Self::next_namespace_identity(&mut next)?;
        next.next_inode = next.next_inode.checked_add(1).ok_or_else(|| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic directory inode identity exhausted",
            )
        })?;
        let key = CloudItemKey::new(
            request.parent_key().scope().clone(),
            CloudItemId::new(format!("local-created-{sequence}")).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic directory item identity was invalid",
                )
            })?,
        );
        let item = CloudItem::directory(
            key.clone(),
            Some(request.parent_key().item_id().clone()),
            request.name(),
            MetadataRevision::from_slice(format!("local-directory-m{sequence}").as_bytes())
                .map_err(|_| {
                    namespace_store_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "synthetic directory metadata revision was invalid",
                    )
                })?,
        )
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic directory item was invalid",
            )
        })?;
        let record = LinuxInodeRecord::new(
            key.clone(),
            LinuxInode::new(next.next_inode).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic directory inode was invalid",
                )
            })?,
            LinuxInodeGeneration::new(sequence).map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic directory inode generation was invalid",
                )
            })?,
        );
        let intent = Self::namespace_intent(
            sequence,
            request.session_generation(),
            DesiredMutation::create(
                request.parent_key().scope().clone(),
                request.parent_key().item_id().clone(),
                request.name(),
                CloudItemKind::Directory,
            )
            .map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic directory mutation was invalid",
                )
            })?,
            MutationPreconditions::default(),
        )?;
        let namespace_item = LinuxNamespaceItem::new(item, record, intent.clone());
        Self::insert_namespace_mutation(&mut next, intent)?;
        next.namespace_names.insert(
            (
                request.parent_key().item_id().clone(),
                request.name().to_owned(),
            ),
            key.clone(),
        );
        next.namespace_items.insert(key, namespace_item.clone());
        self.persist(&next).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })?;
        *state = next;
        drop(state);
        self.mutation_notify.notify_one();
        Ok(namespace_item)
    }

    async fn rename(
        &self,
        request: &LinuxRenameRequest,
    ) -> LinuxNamespaceStoreResult<LinuxRenameAcceptance> {
        let mut state = lock(&self.state);
        Self::namespace_accepting(&state, request.session_generation())?;
        let (current, record) =
            Self::current_namespace_item(&state, request.key())?.ok_or_else(|| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::NotFound,
                    "synthetic rename source was not found",
                )
            })?;
        let current_kind = match current.kind() {
            CloudItemKind::File => LinuxNodeKind::File,
            CloudItemKind::Directory => LinuxNodeKind::Directory,
        };
        if record.generation() != request.inode_generation()
            || current.parent_id() != Some(request.old_parent_key().item_id())
            || current.name() != request.old_name()
            || current_kind != request.kind()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic rename source facts were stale",
            ));
        }
        let destination_key = Self::current_name_key(
            &state,
            request.new_parent_key().item_id(),
            request.new_name(),
        )?;
        if request.no_replace() && destination_key.is_some() {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
                "synthetic rename destination already exists",
            ));
        }
        if destination_key.as_ref().map(CloudItemKey::item_id)
            != request
                .destination()
                .map(|destination| destination.key().item_id())
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic rename destination facts were stale",
            ));
        }
        let mut next = state.clone();
        let sequence = Self::next_namespace_identity(&mut next)?;
        let intent = Self::namespace_intent(
            sequence,
            request.session_generation(),
            DesiredMutation::modify_metadata(
                request.key().clone(),
                Some(request.new_parent_key().item_id().clone()),
                Some(request.new_name().to_owned()),
            )
            .map_err(|_| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "synthetic rename mutation was invalid",
                )
            })?,
            MutationPreconditions::new(
                Some(current.metadata_revision().clone()),
                current.content().map(|content| content.revision().clone()),
            ),
        )?;
        let renamed = match current.kind() {
            CloudItemKind::File => CloudItem::file(
                current.key().clone(),
                request.new_parent_key().item_id().clone(),
                request.new_name(),
                MetadataRevision::from_slice(format!("local-rename-m{sequence}").as_bytes())
                    .map_err(|_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic rename metadata revision was invalid",
                        )
                    })?,
                current.content().cloned().ok_or_else(|| {
                    namespace_store_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "synthetic renamed file omitted content metadata",
                    )
                })?,
            ),
            CloudItemKind::Directory => CloudItem::directory(
                current.key().clone(),
                Some(request.new_parent_key().item_id().clone()),
                request.new_name(),
                MetadataRevision::from_slice(format!("local-rename-m{sequence}").as_bytes())
                    .map_err(|_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic rename metadata revision was invalid",
                        )
                    })?,
            ),
        }
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic renamed item was invalid",
            )
        })?;
        let namespace_item = LinuxNamespaceItem::new(renamed, record.clone(), intent.clone());
        let source = LinuxNamespaceTombstone::new(
            request.key().clone(),
            request.inode_generation(),
            request.old_parent_key().clone(),
            request.old_name(),
            request.kind(),
            intent.clone(),
        )
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic rename source tombstone was invalid",
            )
        })?;
        let replaced = match destination_key {
            Some(destination_key) => {
                let (destination, destination_record) =
                    Self::current_namespace_item(&state, &destination_key)?.ok_or_else(|| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::NotFound,
                            "synthetic rename destination disappeared",
                        )
                    })?;
                let destination_kind = match destination.kind() {
                    CloudItemKind::File => LinuxNodeKind::File,
                    CloudItemKind::Directory => LinuxNodeKind::Directory,
                };
                if destination_kind != request.kind() {
                    return Err(namespace_store_error(
                        match request.kind() {
                            LinuxNodeKind::File => {
                                LinuxNamespaceMutationStoreErrorKind::IsDirectory
                            }
                            LinuxNodeKind::Directory => {
                                LinuxNamespaceMutationStoreErrorKind::NotDirectory
                            }
                        },
                        "synthetic rename replacement kind mismatch",
                    ));
                }
                if destination_kind == LinuxNodeKind::Directory
                    && !Self::directory_is_empty(&state, &destination_key)?
                {
                    return Err(namespace_store_error(
                        LinuxNamespaceMutationStoreErrorKind::DirectoryNotEmpty,
                        "synthetic rename destination directory was not empty",
                    ));
                }
                Some(
                    LinuxNamespaceTombstone::new(
                        destination_key,
                        destination_record.generation(),
                        request.new_parent_key().clone(),
                        request.new_name(),
                        destination_kind,
                        intent.clone(),
                    )
                    .map_err(|_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic replacement tombstone was invalid",
                        )
                    })?,
                )
            }
            None => None,
        };
        Self::insert_namespace_mutation(&mut next, intent)?;
        let old_name_key = (
            request.old_parent_key().item_id().clone(),
            request.old_name().to_owned(),
        );
        let new_name_key = (
            request.new_parent_key().item_id().clone(),
            request.new_name().to_owned(),
        );
        next.namespace_names.remove(&old_name_key);
        next.created_names.remove(&old_name_key);
        next.tombstones.insert(old_name_key, source.clone());
        if let Some(replaced) = replaced.as_ref() {
            next.namespace_items.remove(replaced.key());
            next.created_names.remove(&new_name_key);
            next.namespace_names.remove(&new_name_key);
            next.tombstones
                .insert(new_name_key.clone(), replaced.clone());
        }
        next.namespace_names
            .insert(new_name_key.clone(), request.key().clone());
        if next.created.contains_key(request.key()) {
            next.created_names
                .insert(new_name_key, request.key().clone());
        }
        next.namespace_items
            .insert(request.key().clone(), namespace_item.clone());
        self.persist(&next).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })?;
        *state = next;
        drop(state);
        self.mutation_notify.notify_one();
        Ok(LinuxRenameAcceptance::new(namespace_item, source, replaced))
    }

    async fn remove(
        &self,
        request: &LinuxRemoveRequest,
    ) -> LinuxNamespaceStoreResult<LinuxNamespaceTombstone> {
        let mut state = lock(&self.state);
        Self::namespace_accepting(&state, request.session_generation())?;
        let (current, record) =
            Self::current_namespace_item(&state, request.key())?.ok_or_else(|| {
                namespace_store_error(
                    LinuxNamespaceMutationStoreErrorKind::NotFound,
                    "synthetic remove target was not found",
                )
            })?;
        let current_kind = match current.kind() {
            CloudItemKind::File => LinuxNodeKind::File,
            CloudItemKind::Directory => LinuxNodeKind::Directory,
        };
        if record.generation() != request.inode_generation()
            || current.parent_id() != Some(request.parent_key().item_id())
            || current.name() != request.name()
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic remove facts were stale",
            ));
        }
        if current_kind != request.kind() {
            return Err(namespace_store_error(
                match request.kind() {
                    LinuxNodeKind::File => LinuxNamespaceMutationStoreErrorKind::IsDirectory,
                    LinuxNodeKind::Directory => LinuxNamespaceMutationStoreErrorKind::NotDirectory,
                },
                "synthetic remove target kind mismatch",
            ));
        }
        if current_kind == LinuxNodeKind::Directory
            && !Self::directory_is_empty(&state, request.key())?
        {
            return Err(namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::DirectoryNotEmpty,
                "synthetic directory was not empty",
            ));
        }
        let mut next = state.clone();
        let sequence = Self::next_namespace_identity(&mut next)?;
        let intent = Self::namespace_intent(
            sequence,
            request.session_generation(),
            DesiredMutation::delete(request.key().clone()),
            MutationPreconditions::new(
                Some(current.metadata_revision().clone()),
                current.content().map(|content| content.revision().clone()),
            ),
        )?;
        let tombstone = LinuxNamespaceTombstone::new(
            request.key().clone(),
            request.inode_generation(),
            request.parent_key().clone(),
            request.name(),
            request.kind(),
            intent.clone(),
        )
        .map_err(|_| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "synthetic remove tombstone was invalid",
            )
        })?;
        Self::insert_namespace_mutation(&mut next, intent)?;
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        next.namespace_items.remove(request.key());
        next.namespace_names.remove(&name_key);
        next.created_names.remove(&name_key);
        next.tombstones.insert(name_key, tombstone.clone());
        self.persist(&next).map_err(|error| {
            namespace_store_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })?;
        *state = next;
        drop(state);
        self.mutation_notify.notify_one();
        Ok(tombstone)
    }
}
