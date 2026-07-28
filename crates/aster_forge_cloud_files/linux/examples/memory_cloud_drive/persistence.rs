impl MemoryWritebackStore {
    fn register_mutation_intent(
        state: &mut MemoryWritebackState,
        intents: &mut HashMap<OperationId, MutationIntent>,
        intent: MutationIntent,
    ) -> ExampleResult<()> {
        let operation_id = intent.operation_id().clone();
        let idempotency_key = intent.idempotency_key().clone();
        if let Some(existing) = intents.get(&operation_id) {
            if existing == &intent {
                return Ok(());
            }
            return Err(format!(
                "synthetic namespace reused mutation {} with different intents: existing={existing:?}, restored={intent:?}",
                operation_id.as_str(),
            )
            .into());
        }
        if state
            .idempotency_keys
            .get(&idempotency_key)
            .is_some_and(|existing| existing != &operation_id)
        {
            return Err("synthetic namespace reused a mutation idempotency key".into());
        }
        state
            .idempotency_keys
            .insert(idempotency_key, operation_id.clone());
        intents.insert(operation_id, intent);
        Ok(())
    }

    fn new(scope: CloudScope, state_directory: Option<&Path>) -> ExampleResult<Self> {
        let state_file = state_directory.map(|directory| directory.join("writeback.json"));
        let mut state = MemoryWritebackState::default();
        if let Some(path) = state_file.as_ref() {
            let document = match fs::read(path) {
                Ok(bytes) => serde_json::from_slice::<DurableWritebackDocument>(&bytes)?,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    DurableWritebackDocument::default()
                }
                Err(error) => return Err(Box::new(error)),
            };
            state.active_generation = document
                .active_generation
                .map(SessionGeneration::new)
                .transpose()?;
            state.active_session_state = document.active_session_state.map(SessionState::from);
            if state.active_generation.is_some() && state.active_session_state.is_none() {
                state.active_session_state = Some(SessionState::Accepting);
            }
            state.next_generation = document.next_generation;
            state.next_item = document.next_item;
            state.next_inode = document.next_inode;
            let mut intents = HashMap::new();
            for persisted in document.created {
                let persisted_scope = CloudScope::new(
                    CloudNamespaceId::new(persisted.namespace_id)?,
                    CloudRootId::new(persisted.root_id)?,
                );
                if persisted_scope != scope {
                    return Err(
                        "synthetic writeback state belonged to another cloud scope".into()
                    );
                }
                let key = CloudItemKey::new(
                    persisted_scope.clone(),
                    CloudItemId::new(persisted.item_id)?,
                );
                let parent_id = CloudItemId::new(persisted.parent_id)?;
                let item = CloudItem::file(
                    key.clone(),
                    parent_id.clone(),
                    persisted.name.clone(),
                    MetadataRevision::from_slice(&persisted.metadata_revision)?,
                    CloudContentMetadata::new(
                        ContentRevision::from_slice(&persisted.content_revision)?,
                        None,
                        0,
                    ),
                )?;
                let record = LinuxInodeRecord::new(
                    key.clone(),
                    LinuxInode::new(persisted.inode)?,
                    LinuxInodeGeneration::new(persisted.inode_generation)?,
                );
                let generation = SessionGeneration::new(persisted.accepted_generation)?;
                let intent = MutationIntent::new(
                    OperationId::new(persisted.operation_id.clone())?,
                    IdempotencyKey::new(persisted.idempotency_key.clone())?,
                    MutationOrigin::PlatformCommand,
                    generation,
                    DesiredMutation::create(
                        persisted_scope,
                        parent_id.clone(),
                        persisted.name.clone(),
                        CloudItemKind::File,
                    )?,
                    MutationPreconditions::default(),
                );
                Self::register_mutation_intent(&mut state, &mut intents, intent.clone())?;
                let created = LinuxCreatedFile::new(item, record, intent);
                state.next_item = state.next_item.max(persisted.inode_generation);
                state.next_inode = state.next_inode.max(persisted.inode);
                if state
                    .created_names
                    .insert((parent_id, persisted.name), key.clone())
                    .is_some()
                    || state.created.insert(key, created).is_some()
                {
                    return Err(
                        "synthetic namespace state contained duplicate create records".into(),
                    );
                }
            }
            for persisted in document.namespace_items {
                let item = persisted.into_item(&scope)?;
                let parent_id = item
                    .item()
                    .parent_id()
                    .ok_or("synthetic namespace item omitted its parent")?
                    .clone();
                let key = item.item().key().clone();
                let name = item.item().name().to_owned();
                state.next_inode = state.next_inode.max(item.inode_record().inode().get());
                state.next_item = state
                    .next_item
                    .max(item.inode_record().generation().get());
                Self::register_mutation_intent(
                    &mut state,
                    &mut intents,
                    item.mutation_intent().clone(),
                )?;
                if state
                    .namespace_names
                    .insert((parent_id.clone(), name.clone()), key.clone())
                    .is_some()
                    || state.namespace_items.insert(key.clone(), item).is_some()
                {
                    return Err(
                        "synthetic namespace state contained duplicate overlay items".into(),
                    );
                }
                if state.created.contains_key(&key) {
                    state
                        .created_names
                        .retain(|_, created_key| created_key != &key);
                    state.created_names.insert((parent_id, name), key);
                }
            }
            for persisted in document.tombstones {
                let tombstone = persisted.into_tombstone(&scope)?;
                let name_key = (
                    tombstone.parent_key().item_id().clone(),
                    tombstone.name().to_owned(),
                );
                Self::register_mutation_intent(
                    &mut state,
                    &mut intents,
                    tombstone.mutation_intent().clone(),
                )?;
                if matches!(
                    tombstone.mutation_intent().desired(),
                    DesiredMutation::Delete { key } if key == tombstone.key()
                ) {
                    state
                        .created_names
                        .retain(|_, created_key| created_key != tombstone.key());
                }
                if state.tombstones.insert(name_key, tombstone).is_some() {
                    return Err(
                        "synthetic namespace state contained duplicate tombstones".into(),
                    );
                }
            }
            for persisted in document.mutations {
                let legacy_operation_id = OperationId::new(persisted.operation_id.clone())?;
                let record = persisted
                    .into_record(&scope, intents.get(&legacy_operation_id).cloned())?;
                Self::register_mutation_intent(
                    &mut state,
                    &mut intents,
                    record.intent().clone(),
                )?;
                let operation_id = record.intent().operation_id().clone();
                if state.mutations.insert(operation_id, record).is_some() {
                    return Err(
                        "synthetic writeback state contained duplicate mutation records".into(),
                    );
                }
            }
            for (operation_id, intent) in intents {
                state
                    .mutations
                    .entry(operation_id)
                    .or_insert(MutationRecord::persist(intent)?);
            }
            for persisted in document.files {
                let persisted_scope = CloudScope::new(
                    CloudNamespaceId::new(persisted.namespace_id)?,
                    CloudRootId::new(persisted.root_id)?,
                );
                if persisted_scope != scope {
                    return Err(
                        "synthetic staging state belonged to another cloud scope".into()
                    );
                }
                let key =
                    CloudItemKey::new(persisted_scope, CloudItemId::new(persisted.item_id)?);
                let base_revision = match persisted.base_revision {
                    Some(revision) => ContentRevision::from_slice(&revision)?,
                    None => state
                        .created
                        .get(&key)
                        .and_then(|created| created.item().content())
                        .map(|content| content.revision().clone())
                        .unwrap_or(ContentRevision::from_slice(
                            format!("legacy-base-{}", key.item_id().as_str()).as_bytes(),
                        )?),
                };
                let size = u64::try_from(persisted.bytes.len())?;
                let snapshot = match (persisted.generation, persisted.local_reference) {
                    (Some(generation), Some(reference)) => {
                        let generation = LocalContentGeneration::new(generation)?;
                        state.next_generation = state.next_generation.max(generation.get());
                        Some(LocalContentSnapshot::new(
                            key.clone(),
                            generation,
                            LocalContentReference::new(reference)?,
                            size,
                            None,
                        ))
                    }
                    (None, None) => None,
                    _ => {
                        return Err(
                            "synthetic staging state split snapshot generation/reference"
                                .into(),
                        );
                    }
                };
                if state
                    .files
                    .insert(
                        key,
                        StagedFile {
                            bytes: persisted.bytes,
                            snapshot,
                            base_revision,
                        },
                    )
                    .is_some()
                {
                    return Err(
                        "synthetic writeback state contained duplicate item keys".into()
                    );
                }
            }
            for persisted in document.immutable_snapshots {
                let persisted_scope = CloudScope::new(
                    CloudNamespaceId::new(persisted.namespace_id)?,
                    CloudRootId::new(persisted.root_id)?,
                );
                if persisted_scope != scope {
                    return Err(
                        "synthetic immutable snapshot belonged to another cloud scope".into(),
                    );
                }
                let key =
                    CloudItemKey::new(persisted_scope, CloudItemId::new(persisted.item_id)?);
                let generation = LocalContentGeneration::new(persisted.generation)?;
                state.next_generation = state.next_generation.max(generation.get());
                let reference = LocalContentReference::new(persisted.local_reference)?;
                if state
                    .immutable_snapshots
                    .insert(
                        (key, generation),
                        ImmutableSnapshotBytes {
                            reference,
                            bytes: persisted.bytes,
                        },
                    )
                    .is_some()
                {
                    return Err(
                        "synthetic writeback state contained duplicate immutable generations"
                            .into(),
                    );
                }
            }
            for (key, file) in &state.files {
                if let Some(snapshot) = file.snapshot.as_ref() {
                    let immutable = state
                        .immutable_snapshots
                        .entry((key.clone(), snapshot.generation()))
                        .or_insert_with(|| ImmutableSnapshotBytes {
                            reference: snapshot.reference().clone(),
                            bytes: file.bytes.clone(),
                        });
                    if immutable.reference != *snapshot.reference()
                        || immutable.bytes.len() != file.bytes.len()
                    {
                        return Err(
                            "synthetic immutable snapshot did not match staged metadata".into(),
                        );
                    }
                }
            }
            for persisted in document.uploads {
                let record = persisted.into_record(&scope)?;
                let operation_id = record.intent().operation_id().clone();
                let idempotency_key = record.intent().idempotency_key().clone();
                let snapshot = record.intent().snapshot();
                let immutable = state
                    .immutable_snapshots
                    .get(&(snapshot.item_key().clone(), snapshot.generation()))
                    .ok_or("synthetic upload referenced missing immutable bytes")?;
                if immutable.reference != *snapshot.reference()
                    || u64::try_from(immutable.bytes.len())? != snapshot.size()
                {
                    return Err("synthetic upload snapshot metadata drifted".into());
                }
                if state
                    .upload_idempotency_keys
                    .insert(idempotency_key, operation_id.clone())
                    .is_some()
                    || state.uploads.insert(operation_id, record).is_some()
                {
                    return Err(
                        "synthetic upload state contained duplicate operation identities"
                            .into(),
                    );
                }
            }
            let accepted_generation = state
                .active_generation
                .unwrap_or(SessionGeneration::new(1)?);
            let dirty_without_upload = state
                .files
                .iter()
                .filter_map(|(key, file)| file.snapshot.as_ref().map(|snapshot| (key, file, snapshot)))
                .filter(|(_, _, snapshot)| {
                    !state.uploads.values().any(|record| {
                        record.intent().snapshot().item_key() == snapshot.item_key()
                            && record.intent().snapshot().generation() == snapshot.generation()
                    })
                })
                .map(|(key, file, snapshot)| {
                    (key.clone(), file.base_revision.clone(), snapshot.clone())
                })
                .collect::<Vec<_>>();
            for (key, base_revision, snapshot) in dirty_without_upload {
                let suffix = format!(
                    "{}-{}",
                    key.item_id().as_str(),
                    snapshot.generation().get()
                );
                let intent = ContentUploadIntent::new(
                    OperationId::new(format!("legacy-upload-op-{suffix}"))?,
                    IdempotencyKey::new(format!("legacy-upload-key-{suffix}"))?,
                    ContentCacheKey::new(key, base_revision),
                    snapshot,
                    ContentLeaseId::new(format!("legacy-upload-lease-{suffix}"))?,
                    accepted_generation,
                )?;
                state.upload_idempotency_keys.insert(
                    intent.idempotency_key().clone(),
                    intent.operation_id().clone(),
                );
                state.uploads.insert(
                    intent.operation_id().clone(),
                    ContentUploadRecord::persist(intent),
                );
            }
        }
        Ok(Self {
            scope,
            state: Mutex::new(state),
            state_file,
            mutation_notify: Notify::new(),
            upload_notify: Notify::new(),
        })
    }

    fn next_mount_generation(&self) -> ExampleResult<SessionGeneration> {
        let next = lock(&self.state)
            .active_generation
            .map(SessionGeneration::get)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("synthetic mount generation exhausted")?;
        Ok(SessionGeneration::new(next)?)
    }

    fn reserve_fixture_inodes_through(&self, inode: u64) {
        let mut state = lock(&self.state);
        state.next_inode = state.next_inode.max(inode);
    }

    fn persist(&self, state: &MemoryWritebackState) -> StoreResult<()> {
        let Some(state_file) = self.state_file.as_ref() else {
            return Ok(());
        };
        let mut files = state
            .files
            .iter()
            .map(|(key, file)| DurableStagedFile {
                namespace_id: key.scope().namespace_id().as_str().to_owned(),
                root_id: key.scope().root_id().as_str().to_owned(),
                item_id: key.item_id().as_str().to_owned(),
                bytes: file.bytes.clone(),
                generation: file
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.generation().get()),
                local_reference: file
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.reference().as_str().to_owned()),
                base_revision: Some(file.base_revision.as_bytes().to_vec()),
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| {
            (&left.namespace_id, &left.root_id, &left.item_id).cmp(&(
                &right.namespace_id,
                &right.root_id,
                &right.item_id,
            ))
        });
        let mut created = state
            .created
            .values()
            .map(|created| {
                let item = created.item();
                let content = item.content().ok_or_else(|| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic created file omitted content metadata",
                    )
                })?;
                let parent_id = item.parent_id().ok_or_else(|| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic created file omitted parent identity",
                    )
                })?;
                Ok(DurableCreatedFile {
                    namespace_id: item.key().scope().namespace_id().as_str().to_owned(),
                    root_id: item.key().scope().root_id().as_str().to_owned(),
                    item_id: item.key().item_id().as_str().to_owned(),
                    parent_id: parent_id.as_str().to_owned(),
                    name: item.name().to_owned(),
                    metadata_revision: item.metadata_revision().as_bytes().to_vec(),
                    content_revision: content.revision().as_bytes().to_vec(),
                    inode: created.inode_record().inode().get(),
                    inode_generation: created.inode_record().generation().get(),
                    operation_id: created.mutation_intent().operation_id().as_str().to_owned(),
                    idempotency_key: created
                        .mutation_intent()
                        .idempotency_key()
                        .as_str()
                        .to_owned(),
                    accepted_generation: created.mutation_intent().session_generation().get(),
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        created.sort_by(|left, right| {
            (&left.namespace_id, &left.root_id, &left.item_id).cmp(&(
                &right.namespace_id,
                &right.root_id,
                &right.item_id,
            ))
        });
        let mut namespace_items = state
            .namespace_items
            .values()
            .map(DurableNamespaceItem::from_item)
            .collect::<StoreResult<Vec<_>>>()?;
        namespace_items.sort_by(|left, right| {
            (&left.namespace_id, &left.root_id, &left.item_id).cmp(&(
                &right.namespace_id,
                &right.root_id,
                &right.item_id,
            ))
        });
        let mut tombstones = state
            .tombstones
            .values()
            .map(DurableNamespaceTombstone::from_tombstone)
            .collect::<StoreResult<Vec<_>>>()?;
        tombstones.sort_by(|left, right| {
            (&left.namespace_id, &left.root_id, &left.parent_id, &left.name).cmp(&(
                &right.namespace_id,
                &right.root_id,
                &right.parent_id,
                &right.name,
            ))
        });
        let mut immutable_snapshots = state
            .immutable_snapshots
            .iter()
            .map(|((key, generation), snapshot)| DurableImmutableSnapshot {
                namespace_id: key.scope().namespace_id().as_str().to_owned(),
                root_id: key.scope().root_id().as_str().to_owned(),
                item_id: key.item_id().as_str().to_owned(),
                generation: generation.get(),
                local_reference: snapshot.reference.as_str().to_owned(),
                bytes: snapshot.bytes.clone(),
            })
            .collect::<Vec<_>>();
        immutable_snapshots.sort_by(|left, right| {
            (
                &left.namespace_id,
                &left.root_id,
                &left.item_id,
                left.generation,
            )
                .cmp(&(
                    &right.namespace_id,
                    &right.root_id,
                    &right.item_id,
                    right.generation,
                ))
        });
        let document = DurableWritebackDocument {
            active_generation: state.active_generation.map(SessionGeneration::get),
            active_session_state: state.active_session_state.map(Into::into),
            next_generation: state.next_generation,
            next_item: state.next_item,
            next_inode: state.next_inode,
            files,
            immutable_snapshots,
            created,
            namespace_items,
            tombstones,
            mutations: {
                let mut mutations = state
                    .mutations
                    .values()
                    .map(DurableMutationRecord::from_record)
                    .collect::<StoreResult<Vec<_>>>()?;
                mutations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
                mutations
            },
            uploads: {
                let mut uploads = state
                    .uploads
                    .values()
                    .map(DurableUploadRecord::from_record)
                    .collect::<StoreResult<Vec<_>>>()?;
                uploads.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
                uploads
            },
        };
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("serialize synthetic writeback state: {error}"),
            )
        })?;
        let parent = state_file.parent().ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                "synthetic writeback state path omitted its parent",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("create synthetic writeback directory: {error}"),
            )
        })?;
        let temporary = state_file.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                store_error(
                    CloudFilesStoreErrorKind::PersistenceFailure,
                    format!("open synthetic writeback temporary file: {error}"),
                )
            })?;
        file.write_all(&bytes).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("write synthetic writeback state: {error}"),
            )
        })?;
        file.sync_all().map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("sync synthetic writeback state: {error}"),
            )
        })?;
        fs::rename(&temporary, state_file).map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::PersistenceFailure,
                format!("replace synthetic writeback state: {error}"),
            )
        })?;
        OpenOptions::new()
            .read(true)
            .open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                store_error(
                    CloudFilesStoreErrorKind::PersistenceFailure,
                    format!("sync synthetic writeback directory: {error}"),
                )
            })
    }

    fn session_key(
        state: &MemoryWritebackState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<CloudItemKey> {
        state
            .sessions
            .get(session)
            .map(|binding| binding.key.clone())
            .ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory write session",
                )
            })
    }

    fn require_active_session(
        state: &MemoryWritebackState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<()> {
        let binding = state.sessions.get(session).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory write session",
            )
        })?;
        if state.active_generation != Some(binding.generation)
            || state.active_session_state != Some(SessionState::Accepting)
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "memory write session was fenced by a newer mount",
            ));
        }
        Ok(())
    }

    fn commit(
        &self,
        state: &mut MemoryWritebackState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<LinuxWriteCommit> {
        let binding = state.sessions.get(session).cloned().ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory write session",
            )
        })?;
        let key = binding.key;
        state.next_generation = state.next_generation.checked_add(1).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write generation exhausted",
            )
        })?;
        let generation = LocalContentGeneration::new(state.next_generation).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory write generation was invalid",
            )
        })?;
        let file = state.files.get_mut(&key).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::NotFound,
                "missing memory staged file",
            )
        })?;
        let size = u64::try_from(file.bytes.len()).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "memory staged file length exceeded u64",
            )
        })?;
        let snapshot = LocalContentSnapshot::new(
            key.clone(),
            generation,
            LocalContentReference::new(format!("memory-dirty-{}", generation.get())).map_err(
                |_| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "memory local reference was invalid",
                    )
                },
            )?,
            size,
            None,
        );
        file.snapshot = Some(snapshot.clone());
        let immutable = ImmutableSnapshotBytes {
            reference: snapshot.reference().clone(),
            bytes: file.bytes.clone(),
        };
        let base_revision = file.base_revision.clone();
        if state
            .immutable_snapshots
            .insert((key.clone(), generation), immutable)
            .is_some()
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "memory immutable generation already existed",
            ));
        }
        let operation_id = OperationId::new(format!(
            "content-upload-op-{:020}",
            generation.get()
        ))
        .map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let idempotency_key = IdempotencyKey::new(format!(
            "content-upload-key-{:020}",
            generation.get()
        ))
        .map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        let intent = ContentUploadIntent::new(
            operation_id.clone(),
            idempotency_key.clone(),
            ContentCacheKey::new(key, base_revision),
            snapshot.clone(),
            ContentLeaseId::new(format!(
                "content-upload-lease-{:020}",
                generation.get()
            ))
            .map_err(|error| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    error.to_string(),
                )
            })?,
            binding.generation,
        )
        .map_err(|error| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                error.to_string(),
            )
        })?;
        if state.uploads.contains_key(&operation_id)
            || state
                .upload_idempotency_keys
                .insert(idempotency_key, operation_id.clone())
                .is_some()
        {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "memory upload identity already existed",
            ));
        }
        state
            .uploads
            .insert(operation_id, ContentUploadRecord::persist(intent));
        self.persist(state)?;
        Ok(LinuxWriteCommit::new(snapshot))
    }
}
