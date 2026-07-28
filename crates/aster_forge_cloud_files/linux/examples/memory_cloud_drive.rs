#[cfg(target_os = "linux")]
mod linux_example {
    use std::{
        collections::HashMap,
        error::Error,
        fs::{self, OpenOptions},
        io::{ErrorKind, Write},
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
        sync::{Arc, Mutex, MutexGuard},
        time::Duration,
    };

    use aster_forge_cloud_files_core::{
        Alignment, BackendResult, ChangeCursor, ChangePage, CloudBackendError,
        CloudBackendErrorKind, CloudContentBackend, CloudContentMetadata, CloudFilesBackend,
        CloudFilesCapabilities, CloudFilesStoreError, CloudFilesStoreErrorKind, CloudItem,
        CloudItemId, CloudItemKey, CloudItemKind, CloudItemPage, CloudMetadataBackend,
        CloudMutationBackend, CloudNamespaceId, CloudRootId, CloudScope, ContentReadRange,
        ContentReadRequest, ContentReadResponse, ContentRevision, DesiredMutation,
        EnumerationCapabilities, HydrationCapabilities, IdempotencyKey, IdentityCapabilities,
        LocalContentGeneration, LocalContentReference, LocalContentSnapshot, MetadataRevision,
        MutationCapabilities, MutationIntent, MutationJournalStore, MutationOrigin,
        MutationPreconditions, MutationRecord, MutationRecordTransition, MutationRemoteOutcome,
        MutationRunOutcome, MutationRunner, MutationState, OperationId, PageCursor,
        RangeHydrationCapabilities, RevisionCapabilities, SessionGeneration, SessionState,
        StoreResult, StoreWriteStatus,
    };
    use aster_forge_cloud_files_linux::{
        LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCreateFileAcceptance, LinuxCreateFileRequest,
        LinuxCreatedFile, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord, LinuxInodeTable,
        LinuxMountConfig, LinuxNamespaceMutationStore, LinuxNamespaceMutationStoreError,
        LinuxNamespaceMutationStoreErrorKind, LinuxNamespaceStoreResult, LinuxReadOnlyEngine,
        LinuxWritableEngine, LinuxWritableFilesystem, LinuxWriteCommit, LinuxWriteOpenRequest,
        LinuxWriteSession, LinuxWriteSessionId, LinuxWritebackStore, mount_writable,
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use serde::{Deserialize, Serialize};
    use tokio::sync::{Notify, watch};

    pub type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn store_error(
        kind: CloudFilesStoreErrorKind,
        context: impl Into<String>,
    ) -> CloudFilesStoreError {
        CloudFilesStoreError::new(kind, context)
    }

    fn namespace_store_error(
        kind: LinuxNamespaceMutationStoreErrorKind,
        context: impl Into<String>,
    ) -> LinuxNamespaceMutationStoreError {
        LinuxNamespaceMutationStoreError::new(kind, context)
    }

    #[derive(Clone)]
    struct StagedFile {
        bytes: Vec<u8>,
        snapshot: Option<LocalContentSnapshot>,
    }

    #[derive(Clone, Default)]
    struct MemoryWritebackState {
        active_generation: Option<SessionGeneration>,
        active_session_state: Option<SessionState>,
        next_session: u64,
        next_generation: u64,
        next_item: u64,
        next_inode: u64,
        files: HashMap<CloudItemKey, StagedFile>,
        sessions: HashMap<LinuxWriteSessionId, (CloudItemKey, SessionGeneration)>,
        created: HashMap<CloudItemKey, LinuxCreatedFile>,
        created_names: HashMap<(CloudItemId, String), CloudItemKey>,
        mutations: HashMap<OperationId, MutationRecord>,
        idempotency_keys: HashMap<IdempotencyKey, OperationId>,
    }

    struct MemoryWritebackStore {
        scope: CloudScope,
        state: Mutex<MemoryWritebackState>,
        state_file: Option<PathBuf>,
        mutation_notify: Notify,
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    #[serde(default)]
    struct DurableWritebackDocument {
        active_generation: Option<u64>,
        active_session_state: Option<DurableSessionState>,
        next_generation: u64,
        next_item: u64,
        next_inode: u64,
        files: Vec<DurableStagedFile>,
        created: Vec<DurableCreatedFile>,
        mutations: Vec<DurableMutationRecord>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct DurableStagedFile {
        namespace_id: String,
        root_id: String,
        item_id: String,
        bytes: Vec<u8>,
        generation: Option<u64>,
        local_reference: Option<String>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct DurableCreatedFile {
        namespace_id: String,
        root_id: String,
        item_id: String,
        parent_id: String,
        name: String,
        metadata_revision: Vec<u8>,
        content_revision: Vec<u8>,
        inode: u64,
        inode_generation: u64,
        operation_id: String,
        idempotency_key: String,
        accepted_generation: u64,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum DurableSessionState {
        Accepting,
        Closing,
        Draining,
        Closed,
    }

    impl From<SessionState> for DurableSessionState {
        fn from(value: SessionState) -> Self {
            match value {
                SessionState::Accepting => Self::Accepting,
                SessionState::Closing => Self::Closing,
                SessionState::Draining => Self::Draining,
                SessionState::Closed => Self::Closed,
            }
        }
    }

    impl From<DurableSessionState> for SessionState {
        fn from(value: DurableSessionState) -> Self {
            match value {
                DurableSessionState::Accepting => Self::Accepting,
                DurableSessionState::Closing => Self::Closing,
                DurableSessionState::Draining => Self::Draining,
                DurableSessionState::Closed => Self::Closed,
            }
        }
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum DurableMutationState {
        IntentPersisted,
        RemoteApplying,
        RemoteOutcomeUnknown,
        RemoteOutcomeKnown,
        PlatformReconciled,
        Completed,
    }

    impl From<MutationState> for DurableMutationState {
        fn from(value: MutationState) -> Self {
            match value {
                MutationState::IntentPersisted => Self::IntentPersisted,
                MutationState::RemoteApplying => Self::RemoteApplying,
                MutationState::RemoteOutcomeUnknown => Self::RemoteOutcomeUnknown,
                MutationState::RemoteOutcomeKnown => Self::RemoteOutcomeKnown,
                MutationState::PlatformReconciled => Self::PlatformReconciled,
                MutationState::Completed => Self::Completed,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DurableMutationFile {
        namespace_id: String,
        root_id: String,
        item_id: String,
        parent_id: String,
        name: String,
        metadata_revision: Vec<u8>,
        content_revision: Vec<u8>,
        size: u64,
    }

    impl DurableMutationFile {
        fn from_item(item: &CloudItem) -> StoreResult<Self> {
            let parent_id = item.parent_id().ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic mutation outcome omitted file parent",
                )
            })?;
            let content = item.content().ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic mutation outcome omitted file content metadata",
                )
            })?;
            Ok(Self {
                namespace_id: item.key().scope().namespace_id().as_str().to_owned(),
                root_id: item.key().scope().root_id().as_str().to_owned(),
                item_id: item.key().item_id().as_str().to_owned(),
                parent_id: parent_id.as_str().to_owned(),
                name: item.name().to_owned(),
                metadata_revision: item.metadata_revision().as_bytes().to_vec(),
                content_revision: content.revision().as_bytes().to_vec(),
                size: content.size(),
            })
        }

        fn into_item(self) -> aster_forge_cloud_files_core::Result<CloudItem> {
            CloudItem::file(
                CloudItemKey::new(
                    CloudScope::new(
                        CloudNamespaceId::new(self.namespace_id)?,
                        CloudRootId::new(self.root_id)?,
                    ),
                    CloudItemId::new(self.item_id)?,
                ),
                CloudItemId::new(self.parent_id)?,
                self.name,
                MetadataRevision::from_slice(&self.metadata_revision)?,
                CloudContentMetadata::new(
                    ContentRevision::from_slice(&self.content_revision)?,
                    None,
                    self.size,
                ),
            )
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum DurableMutationOutcome {
        Committed {
            item: Option<DurableMutationFile>,
        },
        AlreadyCommitted {
            item: Option<DurableMutationFile>,
        },
        PreconditionFailed {
            metadata_revision: Option<Vec<u8>>,
            content_revision: Option<Vec<u8>>,
        },
        RemoteOutcomeUnknown,
    }

    impl DurableMutationOutcome {
        fn from_outcome(outcome: &MutationRemoteOutcome) -> StoreResult<Self> {
            Ok(match outcome {
                MutationRemoteOutcome::Committed { item } => Self::Committed {
                    item: item
                        .as_ref()
                        .map(DurableMutationFile::from_item)
                        .transpose()?,
                },
                MutationRemoteOutcome::AlreadyCommitted { item } => Self::AlreadyCommitted {
                    item: item
                        .as_ref()
                        .map(DurableMutationFile::from_item)
                        .transpose()?,
                },
                MutationRemoteOutcome::PreconditionFailed {
                    metadata_revision,
                    content_revision,
                } => Self::PreconditionFailed {
                    metadata_revision: metadata_revision
                        .as_ref()
                        .map(|revision| revision.as_bytes().to_vec()),
                    content_revision: content_revision
                        .as_ref()
                        .map(|revision| revision.as_bytes().to_vec()),
                },
                MutationRemoteOutcome::RemoteOutcomeUnknown => Self::RemoteOutcomeUnknown,
            })
        }

        fn into_outcome(self) -> aster_forge_cloud_files_core::Result<MutationRemoteOutcome> {
            Ok(match self {
                Self::Committed { item } => MutationRemoteOutcome::Committed {
                    item: item.map(DurableMutationFile::into_item).transpose()?,
                },
                Self::AlreadyCommitted { item } => MutationRemoteOutcome::AlreadyCommitted {
                    item: item.map(DurableMutationFile::into_item).transpose()?,
                },
                Self::PreconditionFailed {
                    metadata_revision,
                    content_revision,
                } => MutationRemoteOutcome::PreconditionFailed {
                    metadata_revision: metadata_revision
                        .map(|revision| MetadataRevision::from_slice(&revision))
                        .transpose()?,
                    content_revision: content_revision
                        .map(|revision| ContentRevision::from_slice(&revision))
                        .transpose()?,
                },
                Self::RemoteOutcomeUnknown => MutationRemoteOutcome::RemoteOutcomeUnknown,
            })
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DurableMutationRecord {
        operation_id: String,
        state: DurableMutationState,
        remote_outcome: Option<DurableMutationOutcome>,
    }

    impl DurableMutationRecord {
        fn from_record(record: &MutationRecord) -> StoreResult<Self> {
            Ok(Self {
                operation_id: record.intent().operation_id().as_str().to_owned(),
                state: record.state().into(),
                remote_outcome: record
                    .remote_outcome()
                    .map(DurableMutationOutcome::from_outcome)
                    .transpose()?,
            })
        }

        fn into_record(self, intent: MutationIntent) -> ExampleResult<MutationRecord> {
            if self.operation_id != intent.operation_id().as_str() {
                return Err("synthetic mutation record operation identity drifted".into());
            }
            let mut record = MutationRecord::persist(intent)?;
            if matches!(self.state, DurableMutationState::IntentPersisted) {
                if self.remote_outcome.is_some() {
                    return Err("intent-persisted mutation unexpectedly stored an outcome".into());
                }
                return Ok(record);
            }
            record.begin_remote_apply()?;
            if matches!(self.state, DurableMutationState::RemoteApplying) {
                if self.remote_outcome.is_some() {
                    return Err("remote-applying mutation unexpectedly stored an outcome".into());
                }
                return Ok(record);
            }
            let outcome = self
                .remote_outcome
                .ok_or("advanced synthetic mutation omitted its remote outcome")?
                .into_outcome()?;
            record.record_remote_outcome(outcome)?;
            match self.state {
                DurableMutationState::RemoteOutcomeUnknown
                | DurableMutationState::RemoteOutcomeKnown => {}
                DurableMutationState::PlatformReconciled => {
                    record.mark_platform_reconciled()?;
                }
                DurableMutationState::Completed => {
                    record.mark_platform_reconciled()?;
                    record.complete()?;
                }
                DurableMutationState::IntentPersisted | DurableMutationState::RemoteApplying => {
                    return Err("synthetic mutation replay state was inconsistent".into());
                }
            }
            if record.state()
                != match self.state {
                    DurableMutationState::IntentPersisted => MutationState::IntentPersisted,
                    DurableMutationState::RemoteApplying => MutationState::RemoteApplying,
                    DurableMutationState::RemoteOutcomeUnknown => {
                        MutationState::RemoteOutcomeUnknown
                    }
                    DurableMutationState::RemoteOutcomeKnown => MutationState::RemoteOutcomeKnown,
                    DurableMutationState::PlatformReconciled => MutationState::PlatformReconciled,
                    DurableMutationState::Completed => MutationState::Completed,
                }
            {
                return Err("synthetic mutation outcome did not match its durable state".into());
            }
            Ok(record)
        }
    }

    impl MemoryWritebackStore {
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
                    if intents
                        .insert(intent.operation_id().clone(), intent.clone())
                        .is_some()
                        || state
                            .idempotency_keys
                            .insert(
                                intent.idempotency_key().clone(),
                                intent.operation_id().clone(),
                            )
                            .is_some()
                    {
                        return Err(
                            "synthetic namespace state contained duplicate mutation identities"
                                .into(),
                        );
                    }
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
                for persisted in document.mutations {
                    let operation_id = OperationId::new(persisted.operation_id.clone())?;
                    let intent = intents.get(&operation_id).cloned().ok_or_else(|| {
                        format!(
                            "synthetic mutation {} had no durable local create",
                            operation_id.as_str()
                        )
                    })?;
                    let record = persisted.into_record(intent)?;
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
                            },
                        )
                        .is_some()
                    {
                        return Err(
                            "synthetic writeback state contained duplicate item keys".into()
                        );
                    }
                }
            }
            Ok(Self {
                scope,
                state: Mutex::new(state),
                state_file,
                mutation_notify: Notify::new(),
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
            let document = DurableWritebackDocument {
                active_generation: state.active_generation.map(SessionGeneration::get),
                active_session_state: state.active_session_state.map(Into::into),
                next_generation: state.next_generation,
                next_item: state.next_item,
                next_inode: state.next_inode,
                files,
                created,
                mutations: {
                    let mut mutations = state
                        .mutations
                        .values()
                        .map(DurableMutationRecord::from_record)
                        .collect::<StoreResult<Vec<_>>>()?;
                    mutations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
                    mutations
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
                .map(|(key, _)| key.clone())
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
            let (_, generation) = state.sessions.get(session).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory write session",
                )
            })?;
            if state.active_generation != Some(*generation) {
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
            let key = Self::session_key(state, session)?;
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
                key,
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
            self.persist(state)?;
            Ok(LinuxWriteCommit::new(snapshot))
        }
    }

    #[async_trait]
    impl LinuxWritebackStore for MemoryWritebackStore {
        async fn activate_mount(
            &self,
            scope: &CloudScope,
            session_generation: SessionGeneration,
        ) -> StoreResult<Vec<LocalContentSnapshot>> {
            let mut state = lock(&self.state);
            if state
                .active_generation
                .is_some_and(|current| session_generation < current)
            {
                return Err(store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "synthetic mount generation regressed",
                ));
            }
            state.active_generation = Some(session_generation);
            self.persist(&state)?;
            Ok(state
                .files
                .iter()
                .filter(|(key, file)| key.scope() == scope && file.snapshot.is_some())
                .filter_map(|(_, file)| file.snapshot.clone())
                .collect())
        }

        async fn open_recovered(
            &self,
            key: &CloudItemKey,
            session_generation: SessionGeneration,
        ) -> StoreResult<Option<LinuxWriteSession>> {
            let mut state = lock(&self.state);
            if state.active_generation != Some(session_generation) {
                return Err(store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "recovered open used a fenced synthetic mount",
                ));
            }
            let Some(snapshot) = state.files.get(key).and_then(|file| file.snapshot.clone()) else {
                return Ok(None);
            };
            state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write session exhausted",
                )
            })?;
            let session =
                LinuxWriteSessionId::new(format!("memory-session-{}", state.next_session))
                    .map_err(|_| {
                        store_error(
                            CloudFilesStoreErrorKind::InvalidTransition,
                            "memory write session id was invalid",
                        )
                    })?;
            state
                .sessions
                .insert(session.clone(), (key.clone(), session_generation));
            Ok(Some(LinuxWriteSession::from_recovered(
                session,
                snapshot,
                session_generation,
            )))
        }

        async fn open_existing(
            &self,
            request: &LinuxWriteOpenRequest,
            base_content: Bytes,
        ) -> StoreResult<LinuxWriteSession> {
            let base_size = u64::try_from(base_content.len()).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory base content length exceeded u64",
                )
            })?;
            if base_size != request.base_size() {
                return Err(store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory base content did not match metadata size",
                ));
            }
            let mut state = lock(&self.state);
            if state.active_generation != Some(request.session_generation())
                || state.active_session_state != Some(SessionState::Accepting)
            {
                return Err(store_error(
                    CloudFilesStoreErrorKind::Conflict,
                    "fresh open used a fenced synthetic mount",
                ));
            }
            state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write session exhausted",
                )
            })?;
            let session =
                LinuxWriteSessionId::new(format!("memory-session-{}", state.next_session))
                    .map_err(|_| {
                        store_error(
                            CloudFilesStoreErrorKind::InvalidTransition,
                            "memory write session id was invalid",
                        )
                    })?;
            let file = state
                .files
                .entry(request.key().clone())
                .or_insert_with(|| StagedFile {
                    bytes: base_content.to_vec(),
                    snapshot: None,
                });
            let size = u64::try_from(file.bytes.len()).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory staged file length exceeded u64",
                )
            })?;
            state.sessions.insert(
                session.clone(),
                (request.key().clone(), request.session_generation()),
            );
            Ok(LinuxWriteSession::new(
                session,
                request.key().clone(),
                size,
                request.session_generation(),
            ))
        }

        async fn read(
            &self,
            session: &LinuxWriteSessionId,
            offset: u64,
            size: u32,
        ) -> StoreResult<Bytes> {
            let state = lock(&self.state);
            let key = Self::session_key(&state, session)?;
            let file = state.files.get(&key).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory staged file",
                )
            })?;
            let start = usize::try_from(offset)
                .unwrap_or(file.bytes.len())
                .min(file.bytes.len());
            let end = start.saturating_add(size as usize).min(file.bytes.len());
            Ok(Bytes::copy_from_slice(&file.bytes[start..end]))
        }

        async fn write(
            &self,
            session: &LinuxWriteSessionId,
            offset: u64,
            bytes: Bytes,
        ) -> StoreResult<LinuxWriteCommit> {
            let mut state = lock(&self.state);
            Self::require_active_session(&state, session)?;
            let key = Self::session_key(&state, session)?;
            let file = state.files.get_mut(&key).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory staged file",
                )
            })?;
            let start = usize::try_from(offset).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write offset exceeded usize",
                )
            })?;
            let end = start.checked_add(bytes.len()).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory write end overflowed usize",
                )
            })?;
            if file.bytes.len() < end {
                file.bytes.resize(end, 0);
            }
            file.bytes[start..end].copy_from_slice(&bytes);
            self.commit(&mut state, session)
        }

        async fn truncate(
            &self,
            session: &LinuxWriteSessionId,
            size: u64,
        ) -> StoreResult<LinuxWriteCommit> {
            let mut state = lock(&self.state);
            Self::require_active_session(&state, session)?;
            let key = Self::session_key(&state, session)?;
            let file = state.files.get_mut(&key).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory staged file",
                )
            })?;
            let size = usize::try_from(size).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "memory truncate size exceeded usize",
                )
            })?;
            file.bytes.resize(size, 0);
            self.commit(&mut state, session)
        }

        async fn sync(
            &self,
            session: &LinuxWriteSessionId,
            _data_only: bool,
        ) -> StoreResult<Option<LinuxWriteCommit>> {
            let state = lock(&self.state);
            Self::require_active_session(&state, session)?;
            let key = Self::session_key(&state, session)?;
            Ok(state
                .files
                .get(&key)
                .and_then(|file| file.snapshot.clone())
                .map(LinuxWriteCommit::new))
        }

        async fn close(&self, session: &LinuxWriteSessionId) -> StoreResult<()> {
            lock(&self.state).sessions.remove(session).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "missing memory write session",
                )
            })?;
            Ok(())
        }
    }

    #[async_trait]
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
            if state.created_names.contains_key(&name_key) {
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
                MetadataRevision::from_slice(
                    format!("local-create-m{}", next.next_item).as_bytes(),
                )
                .map_err(|_| {
                    namespace_store_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "synthetic metadata revision was invalid",
                    )
                })?,
                CloudContentMetadata::new(
                    ContentRevision::from_slice(
                        format!("local-create-c{}", next.next_item).as_bytes(),
                    )
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
                IdempotencyKey::new(format!("local-create-key-{}", next.next_item)).map_err(
                    |_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic idempotency key was invalid",
                        )
                    },
                )?,
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
            let session_id =
                LinuxWriteSessionId::new(format!("memory-session-{}", next.next_session)).map_err(
                    |_| {
                        namespace_store_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "synthetic write session identity was invalid",
                        )
                    },
                )?;
            next.files.insert(
                key.clone(),
                StagedFile {
                    bytes: Vec::new(),
                    snapshot: None,
                },
            );
            next.sessions.insert(
                session_id.clone(),
                (key.clone(), request.session_generation()),
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
            state
                .sessions
                .insert(id.clone(), (key.clone(), session_generation));
            Ok(LinuxWriteSession::new(
                id,
                key.clone(),
                size,
                session_generation,
            ))
        }
    }

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

        fn created_for_operation(
            &self,
            operation_id: &OperationId,
        ) -> StoreResult<(CloudItem, u64)> {
            let state = lock(&self.state);
            let key = Self::created_operation_key(&state, operation_id)?;
            let created = state.created.get(&key).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::NotFound,
                    "synthetic created item disappeared",
                )
            })?;
            let size = state.files.get(&key).map_or(0, |file| file.bytes.len());
            let size = u64::try_from(size).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "synthetic staged size exceeded u64",
                )
            })?;
            Ok((created.item().clone(), size))
        }
    }

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

        async fn recoverable_mutations(
            &self,
            scope: &CloudScope,
        ) -> StoreResult<Vec<MutationRecord>> {
            self.scope_matches(scope)?;
            let state = lock(&self.state);
            let mut records = state
                .mutations
                .values()
                .filter(|record| {
                    record.intent().desired().scope() == scope
                        && record.state() != MutationState::Completed
                })
                .cloned()
                .collect::<Vec<_>>();
            records.sort_by(|left, right| {
                left.intent()
                    .operation_id()
                    .as_str()
                    .cmp(right.intent().operation_id().as_str())
            });
            Ok(records)
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
            let outcome = next
                .mutations
                .get(operation_id)
                .and_then(|record| record.remote_outcome().cloned());
            if let Some(
                MutationRemoteOutcome::Committed { item: Some(item) }
                | MutationRemoteOutcome::AlreadyCommitted { item: Some(item) },
            ) = outcome
            {
                let key = Self::created_operation_key(&next, operation_id)?;
                let created = next.created.get(&key).cloned().ok_or_else(|| {
                    store_error(
                        CloudFilesStoreErrorKind::NotFound,
                        "synthetic platform reconciliation lost its local created item",
                    )
                })?;
                if created.item().key() != item.key() {
                    return Err(store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "synthetic remote create changed the preallocated item identity",
                    ));
                }
                let (_, inode_record, intent) = created.into_parts();
                next.created
                    .insert(key, LinuxCreatedFile::new(item, inode_record, intent));
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

    #[derive(Debug, Default, Serialize, Deserialize)]
    #[serde(default)]
    struct DurableRemoteDocument {
        committed: Vec<DurableRemoteCreate>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct DurableRemoteCreate {
        idempotency_key: String,
        item: DurableMutationFile,
    }

    #[derive(Clone, Default)]
    struct RemoteMutationState {
        committed: HashMap<IdempotencyKey, CloudItem>,
    }

    struct MemoryRemoteMutationBackend {
        namespace_store: Arc<MemoryWritebackStore>,
        state: Mutex<RemoteMutationState>,
        state_file: Option<PathBuf>,
        commit_pause: Duration,
    }

    impl MemoryRemoteMutationBackend {
        fn new(
            namespace_store: Arc<MemoryWritebackStore>,
            state_directory: Option<&Path>,
        ) -> ExampleResult<Self> {
            let state_file = state_directory.map(|directory| directory.join("remote.json"));
            let mut state = RemoteMutationState::default();
            if let Some(path) = state_file.as_ref() {
                let document = match fs::read(path) {
                    Ok(bytes) => serde_json::from_slice::<DurableRemoteDocument>(&bytes)?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        DurableRemoteDocument::default()
                    }
                    Err(error) => return Err(Box::new(error)),
                };
                for entry in document.committed {
                    let key = IdempotencyKey::new(entry.idempotency_key)?;
                    if state
                        .committed
                        .insert(key, entry.item.into_item()?)
                        .is_some()
                    {
                        return Err(
                            "synthetic remote ledger contained duplicate idempotency keys".into(),
                        );
                    }
                }
            }
            let pause_ms = std::env::var("ASTER_FORGE_CLOUD_FILES_EXAMPLE_REMOTE_COMMIT_PAUSE_MS")
                .ok()
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or_default();
            Ok(Self {
                namespace_store,
                state: Mutex::new(state),
                state_file,
                commit_pause: Duration::from_millis(pause_ms),
            })
        }

        fn persist(&self, state: &RemoteMutationState) -> ExampleResult<()> {
            let Some(state_file) = self.state_file.as_ref() else {
                return Ok(());
            };
            let mut committed = state
                .committed
                .iter()
                .map(|(idempotency_key, item)| {
                    Ok(DurableRemoteCreate {
                        idempotency_key: idempotency_key.as_str().to_owned(),
                        item: DurableMutationFile::from_item(item)?,
                    })
                })
                .collect::<StoreResult<Vec<_>>>()?;
            committed.sort_by(|left, right| left.idempotency_key.cmp(&right.idempotency_key));
            let bytes = serde_json::to_vec_pretty(&DurableRemoteDocument { committed })?;
            let parent = state_file
                .parent()
                .ok_or("synthetic remote ledger path omitted its parent")?;
            fs::create_dir_all(parent)?;
            let temporary = state_file.with_extension("json.tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, state_file)?;
            OpenOptions::new().read(true).open(parent)?.sync_all()?;
            Ok(())
        }

        fn remote_item(&self, intent: &MutationIntent) -> BackendResult<CloudItem> {
            let (local, size) = self
                .namespace_store
                .created_for_operation(intent.operation_id())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            let (scope, parent_id, name) = match intent.desired() {
                DesiredMutation::Create {
                    scope,
                    parent_id,
                    name,
                    kind: CloudItemKind::File,
                } => (scope, parent_id, name),
                DesiredMutation::Create { .. } => {
                    return Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported));
                }
                _ => return Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported)),
            };
            if local.key().scope() != scope
                || local.parent_id() != Some(parent_id)
                || local.name() != name
            {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidResponse,
                ));
            }
            CloudItem::file(
                local.key().clone(),
                parent_id.clone(),
                name,
                MetadataRevision::from_slice(
                    format!("remote-create-m-{}", intent.operation_id().as_str()).as_bytes(),
                )
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?,
                CloudContentMetadata::new(
                    ContentRevision::from_slice(
                        format!("remote-create-c-{}", intent.operation_id().as_str()).as_bytes(),
                    )
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?,
                    None,
                    size,
                ),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
        }
    }

    #[async_trait]
    impl CloudMutationBackend for MemoryRemoteMutationBackend {
        async fn apply_mutation(
            &self,
            intent: &MutationIntent,
        ) -> BackendResult<MutationRemoteOutcome> {
            let key = intent.idempotency_key().clone();
            if let Some(item) = lock(&self.state).committed.get(&key).cloned() {
                return Ok(MutationRemoteOutcome::AlreadyCommitted { item: Some(item) });
            }
            let item = self.remote_item(intent)?;
            {
                let mut state = lock(&self.state);
                if let Some(existing) = state.committed.get(&key).cloned() {
                    return Ok(MutationRemoteOutcome::AlreadyCommitted {
                        item: Some(existing),
                    });
                }
                let mut next = state.clone();
                next.committed.insert(key, item.clone());
                self.persist(&next).map_err(|_| {
                    CloudBackendError::new(CloudBackendErrorKind::TemporarilyUnavailable)
                })?;
                *state = next;
            }
            if !self.commit_pause.is_zero() {
                tokio::time::sleep(self.commit_pause).await;
            }
            Ok(MutationRemoteOutcome::Committed { item: Some(item) })
        }

        async fn reconcile_mutation(
            &self,
            intent: &MutationIntent,
        ) -> BackendResult<MutationRemoteOutcome> {
            Ok(
                match lock(&self.state)
                    .committed
                    .get(intent.idempotency_key())
                    .cloned()
                {
                    Some(item) => MutationRemoteOutcome::AlreadyCommitted { item: Some(item) },
                    None => MutationRemoteOutcome::RemoteOutcomeUnknown,
                },
            )
        }
    }

    struct MemoryCloud {
        capabilities: CloudFilesCapabilities,
        scope: CloudScope,
        items: HashMap<CloudItemKey, CloudItem>,
        children: HashMap<CloudItemKey, Vec<CloudItemKey>>,
        contents: HashMap<CloudItemKey, (ContentRevision, Bytes)>,
        page_size: usize,
    }

    impl MemoryCloud {
        fn fixture() -> ExampleResult<(Self, Vec<LinuxInodeRecord>)> {
            let scope = CloudScope::new(
                CloudNamespaceId::new("aster-forge-memory-example")?,
                CloudRootId::new("linux-default")?,
            );
            let root_key = key(&scope, "root")?;
            let docs_key = key(&scope, "docs")?;
            let hello_key = key(&scope, "hello")?;
            let readme_key = key(&scope, "readme")?;
            let numbers_key = key(&scope, "numbers")?;
            let guide_key = key(&scope, "guide")?;

            let root = CloudItem::directory(
                root_key.clone(),
                None,
                "AsterForge Memory Cloud",
                metadata_revision("root-m1")?,
            )?;
            let docs = CloudItem::directory(
                docs_key.clone(),
                Some(root_key.item_id().clone()),
                "docs",
                metadata_revision("docs-m1")?,
            )?;
            let hello = file(
                hello_key.clone(),
                &root_key,
                "hello.txt",
                "hello-c1",
                b"Hello from the AsterForge Linux FUSE memory cloud.\n",
            )?;
            let readme = file(
                readme_key.clone(),
                &root_key,
                "readme.txt",
                "readme-c1",
                b"This writable mount uses product-neutral synthetic dirty staging.\n",
            )?;
            let numbers_bytes = (0_u32..4096).flat_map(u32::to_le_bytes).collect::<Vec<_>>();
            let numbers = file(
                numbers_key.clone(),
                &root_key,
                "numbers.bin",
                "numbers-c1",
                &numbers_bytes,
            )?;
            let guide = file(
                guide_key.clone(),
                &docs_key,
                "guide.txt",
                "guide-c1",
                b"Nested directory enumeration and content fetch are active.\n",
            )?;

            let items = HashMap::from([
                (root_key.clone(), root.clone()),
                (docs_key.clone(), docs.clone()),
                (hello_key.clone(), hello.clone()),
                (readme_key.clone(), readme.clone()),
                (numbers_key.clone(), numbers.clone()),
                (guide_key.clone(), guide.clone()),
            ]);
            let children = HashMap::from([
                (
                    root_key.clone(),
                    vec![
                        docs_key.clone(),
                        hello_key.clone(),
                        readme_key.clone(),
                        numbers_key.clone(),
                    ],
                ),
                (docs_key.clone(), vec![guide_key.clone()]),
            ]);
            let contents = [
                (
                    &hello,
                    Bytes::from_static(b"Hello from the AsterForge Linux FUSE memory cloud.\n"),
                ),
                (
                    &readme,
                    Bytes::from_static(
                        b"This writable mount uses product-neutral synthetic dirty staging.\n",
                    ),
                ),
                (&numbers, Bytes::from(numbers_bytes)),
                (
                    &guide,
                    Bytes::from_static(
                        b"Nested directory enumeration and content fetch are active.\n",
                    ),
                ),
            ]
            .into_iter()
            .map(|(item, bytes)| {
                let revision = item
                    .content()
                    .map(|content| content.revision().clone())
                    .ok_or_else(|| {
                        CloudBackendError::new(CloudBackendErrorKind::InvalidResponse)
                    })?;
                Ok((item.key().clone(), (revision, bytes)))
            })
            .collect::<BackendResult<HashMap<_, _>>>()?;

            let capabilities = CloudFilesCapabilities {
                identity: IdentityCapabilities {
                    stable_item_ids: true,
                    path_independent_item_ids: true,
                    same_root_move_preserves_identity: true,
                    cross_root_move_preserves_identity: false,
                },
                revisions: RevisionCapabilities {
                    separate_metadata_and_content: true,
                    conditional_metadata_mutation: false,
                    conditional_content_access: true,
                },
                enumeration: EnumerationCapabilities {
                    paged: true,
                    stable_snapshot: true,
                },
                hydration: HydrationCapabilities {
                    whole_file: true,
                    range: Some(RangeHydrationCapabilities {
                        alignment: Alignment::ONE,
                        partial_cancel: false,
                    }),
                    ..HydrationCapabilities::default()
                },
                mutations: MutationCapabilities::default(),
                ..CloudFilesCapabilities::default()
            };

            let generation = LinuxInodeGeneration::new(1)?;
            let records = [root, docs, hello, readme, numbers, guide]
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    let inode = u64::try_from(index + 1)
                        .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)?;
                    Ok(LinuxInodeRecord::new(
                        item.key().clone(),
                        LinuxInode::new(inode)?,
                        generation,
                    ))
                })
                .collect::<ExampleResult<Vec<_>>>()?;

            Ok((
                Self {
                    capabilities,
                    scope,
                    items,
                    children,
                    contents,
                    page_size: 2,
                },
                records,
            ))
        }

        fn encode_cursor(parent: &CloudItemKey, offset: usize) -> BackendResult<PageCursor> {
            PageCursor::new(format!("{}:{offset}", parent.item_id()).into_bytes())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
        }

        fn decode_cursor(parent: &CloudItemKey, cursor: &PageCursor) -> BackendResult<usize> {
            let value = std::str::from_utf8(cursor.as_bytes())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
            let prefix = format!("{}:", parent.item_id());
            value
                .strip_prefix(&prefix)
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?
                .parse()
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))
        }
    }

    #[async_trait]
    impl CloudMetadataBackend for MemoryCloud {
        async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
            self.items
                .get(key)
                .cloned()
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))
        }

        async fn list_children(
            &self,
            parent: &CloudItemKey,
            cursor: Option<&PageCursor>,
        ) -> BackendResult<CloudItemPage> {
            let parent_item = self
                .items
                .get(parent)
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
            if parent_item.kind() != CloudItemKind::Directory {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidRequest,
                ));
            }
            let keys = self.children.get(parent).map(Vec::as_slice).unwrap_or(&[]);
            let offset = match cursor {
                Some(cursor) => Self::decode_cursor(parent, cursor)?,
                None => 0,
            };
            if offset > keys.len() {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidRequest,
                ));
            }
            let end = offset.saturating_add(self.page_size).min(keys.len());
            let items = keys[offset..end]
                .iter()
                .map(|key| {
                    self.items.get(key).cloned().ok_or_else(|| {
                        CloudBackendError::new(CloudBackendErrorKind::InvalidResponse)
                    })
                })
                .collect::<BackendResult<Vec<_>>>()?;
            let next = if end < keys.len() {
                Some(Self::encode_cursor(parent, end)?)
            } else {
                None
            };
            Ok(CloudItemPage::new(items, next))
        }

        async fn changes_since(
            &self,
            scope: &CloudScope,
            _cursor: Option<&ChangeCursor>,
        ) -> BackendResult<ChangePage> {
            if scope != &self.scope {
                return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
            }
            Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl CloudContentBackend for MemoryCloud {
        async fn read_content(
            &self,
            request: &ContentReadRequest,
        ) -> BackendResult<ContentReadResponse> {
            let Some((revision, content)) = self.contents.get(request.key()) else {
                return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
            };
            if revision != request.revision() {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::PreconditionFailed,
                ));
            }
            let total_size = u64::try_from(content.len())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
            let (offset, bytes) = match request.read_range() {
                ContentReadRange::Whole => (0, content.clone()),
                ContentReadRange::Range(range) => {
                    if range.offset() > total_size {
                        return Err(CloudBackendError::new(
                            CloudBackendErrorKind::InvalidRequest,
                        ));
                    }
                    let start = usize::try_from(range.offset()).map_err(|_| {
                        CloudBackendError::new(CloudBackendErrorKind::InvalidRequest)
                    })?;
                    let end =
                        usize::try_from(range.end_exclusive().min(total_size)).map_err(|_| {
                            CloudBackendError::new(CloudBackendErrorKind::InvalidRequest)
                        })?;
                    (range.offset(), content.slice(start..end))
                }
            };
            let response = ContentReadResponse::new(revision.clone(), offset, bytes, total_size)
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
            request
                .validate_response(&response)
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
            Ok(response)
        }
    }

    impl CloudFilesBackend for MemoryCloud {
        fn capabilities(&self) -> CloudFilesCapabilities {
            self.capabilities
        }
    }

    fn key(scope: &CloudScope, item: &str) -> aster_forge_cloud_files_core::Result<CloudItemKey> {
        Ok(CloudItemKey::new(scope.clone(), CloudItemId::new(item)?))
    }

    fn metadata_revision(value: &str) -> aster_forge_cloud_files_core::Result<MetadataRevision> {
        MetadataRevision::from_slice(value.as_bytes())
    }

    fn content_revision(value: &str) -> aster_forge_cloud_files_core::Result<ContentRevision> {
        ContentRevision::from_slice(value.as_bytes())
    }

    fn file(
        key: CloudItemKey,
        parent: &CloudItemKey,
        name: &str,
        revision: &str,
        bytes: &[u8],
    ) -> aster_forge_cloud_files_core::Result<CloudItem> {
        CloudItem::file(
            key,
            parent.item_id().clone(),
            name,
            metadata_revision(&format!("{revision}-metadata"))?,
            CloudContentMetadata::new(
                content_revision(revision)?,
                None,
                u64::try_from(bytes.len()).map_err(|_| {
                    aster_forge_cloud_files_core::CloudFilesCoreError::InvalidContentResponse {
                        reason: "example content length does not fit u64",
                    }
                })?,
            ),
        )
    }

    async fn run_mutation_worker(
        store: Arc<MemoryWritebackStore>,
        backend: Arc<MemoryRemoteMutationBackend>,
        scope: CloudScope,
        generation: SessionGeneration,
        mut shutdown: watch::Receiver<bool>,
    ) -> ExampleResult<()> {
        let runner = MutationRunner;
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let records = store.recoverable_mutations(&scope).await?;
            if records.is_empty() {
                tokio::select! {
                    _ = store.mutation_notify.notified() => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
                continue;
            }

            let mut retry_later = false;
            for record in records {
                match runner
                    .resume(
                        record.intent().operation_id(),
                        generation,
                        &*store,
                        &*backend,
                    )
                    .await
                {
                    Ok(MutationRunOutcome::Completed) => {}
                    Ok(MutationRunOutcome::RemoteOutcomePending) => retry_later = true,
                    Ok(MutationRunOutcome::Fenced) => return Ok(()),
                    Err(error) => {
                        eprintln!("synthetic mutation worker retry: {error}");
                        retry_later = true;
                    }
                }
            }
            if retry_later {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    pub fn run() -> ExampleResult<()> {
        let mut arguments = std::env::args_os().skip(1);
        let mountpoint = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/aster-forge-memory-cloud"));
        let state_directory = arguments.next().map(PathBuf::from);
        if arguments.next().is_some() {
            return Err("expected at most MOUNTPOINT and optional STATE_DIRECTORY".into());
        }
        std::fs::create_dir_all(&mountpoint)?;
        let metadata = std::fs::metadata(&mountpoint)?;
        let (backend, mut records) = MemoryCloud::fixture()?;
        let scope = backend.scope.clone();
        let root = records.remove(0);
        if root.inode() != LINUX_ROOT_INODE {
            return Err("memory fixture root did not use inode one".into());
        }
        let inode_table = Arc::new(LinuxInodeTable::new(root, records)?);
        let attributes = LinuxAttributePolicy::new(
            metadata.uid(),
            metadata.gid(),
            0o644,
            0o755,
            Duration::from_secs(1),
        )?;
        let store = Arc::new(MemoryWritebackStore::new(
            scope.clone(),
            state_directory.as_deref(),
        )?);
        store.reserve_fixture_inodes_through(6);
        let session_generation = store.next_mount_generation()?;
        let metadata_backend = Arc::new(backend);
        let mutation_backend = Arc::new(MemoryRemoteMutationBackend::new(
            store.clone(),
            state_directory.as_deref(),
        )?);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        runtime.block_on(store.activate_session(&scope, session_generation))?;
        let readonly = LinuxReadOnlyEngine::new(metadata_backend, inode_table, attributes);
        let engine = runtime.block_on(LinuxWritableEngine::activate_with_namespace(
            readonly,
            store.clone(),
            store.clone(),
            session_generation,
        ))?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let worker = runtime.spawn(run_mutation_worker(
            store.clone(),
            mutation_backend,
            scope.clone(),
            session_generation,
            shutdown_rx,
        ));
        let filesystem = LinuxWritableFilesystem::new(engine, runtime.handle().clone(), 64)?;
        let config = LinuxMountConfig::new("aster-forge-memory-cloud")?
            .with_subtype("aster_forge_cloud_files")?;

        println!("mounting writable memory cloud at {}", mountpoint.display());
        println!(
            "unmount from another terminal with: fusermount3 -u {}",
            mountpoint.display()
        );
        match state_directory.as_ref() {
            Some(path) => println!(
                "synthetic dirty generations persist across restarts in {}",
                path.display()
            ),
            None => println!("dirty generations exist only while this example process is running"),
        }
        println!(
            "active synthetic mount generation: {}",
            session_generation.get()
        );
        println!(
            "existing and newly created files support write, truncate, flush, fsync, and reopen"
        );
        println!(
            "new creates use a synthetic remote ledger; rename, unlink, mkdir, and remote change-feed migration remain later work"
        );
        if let Some(path) = state_directory.as_ref() {
            println!(
                "synthetic remote mutation outcomes persist in {}",
                path.join("remote.json").display()
            );
        }
        let mount_result = mount_writable(filesystem, &mountpoint, &config);
        runtime.block_on(store.transition_session(
            &scope,
            session_generation,
            SessionState::Closing,
        ))?;
        let _ = shutdown_tx.send(true);
        runtime.block_on(store.transition_session(
            &scope,
            session_generation,
            SessionState::Draining,
        ))?;
        runtime.block_on(worker)??;
        runtime.block_on(store.transition_session(
            &scope,
            session_generation,
            SessionState::Closed,
        ))?;
        mount_result?;
        Ok(())
    }

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

        #[tokio::test]
        async fn remote_commit_before_outcome_marker_recovers_after_restart() {
            let directory = test_directory("remote-restart");
            fs::create_dir_all(&directory).unwrap();
            let (cloud, _) = MemoryCloud::fixture().unwrap();
            let scope = cloud.scope.clone();
            let generation_one = SessionGeneration::new(1).unwrap();
            let store =
                Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
            let acceptance = accepted_create(&store, &scope, generation_one).await;
            let backend =
                MemoryRemoteMutationBackend::new(store.clone(), Some(&directory)).unwrap();
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
            let store =
                Arc::new(MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap());
            let _acceptance = accepted_create(&store, &scope, generation).await;
            let writeback = directory.join("writeback.json");
            let mut document: DurableWritebackDocument =
                serde_json::from_slice(&fs::read(&writeback).unwrap()).unwrap();
            document.mutations.clear();
            fs::write(&writeback, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

            let recovered = MemoryWritebackStore::new(scope.clone(), Some(&directory)).unwrap();
            let records = recovered.recoverable_mutations(&scope).await.unwrap();
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
                    .recoverable_mutations(&scope)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> linux_example::ExampleResult<()> {
    linux_example::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("memory_cloud_drive requires Linux and a usable /dev/fuse device");
}
