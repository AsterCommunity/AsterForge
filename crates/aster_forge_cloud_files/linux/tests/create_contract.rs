use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use aster_forge_cloud_files_core::{
    BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudFilesBackend, CloudFilesCapabilities,
    CloudFilesStoreError, CloudFilesStoreErrorKind, CloudItem, CloudItemId, CloudItemKey,
    CloudItemPage, CloudMetadataBackend, CloudNamespaceId, CloudRootId, CloudScope,
    ContentReadRange, ContentReadRequest, ContentReadResponse, ContentRevision, DesiredMutation,
    IdempotencyKey, LocalContentGeneration, LocalContentReference, LocalContentSnapshot,
    MetadataRevision, MutationIntent, MutationOrigin, MutationPreconditions, OperationId,
    PageCursor, SessionGeneration, StoreResult,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCloudFilesError, LinuxCreateFileAcceptance,
    LinuxCreateFileRequest, LinuxCreatedFile, LinuxFileAccess, LinuxInode, LinuxInodeGeneration,
    LinuxInodeRecord, LinuxInodeTable, LinuxNamespaceMutationStore,
    LinuxNamespaceMutationStoreError, LinuxNamespaceMutationStoreErrorKind,
    LinuxNamespaceStoreResult, LinuxReadOnlyEngine, LinuxWritableEngine, LinuxWriteCommit,
    LinuxWriteOpenRequest, LinuxWriteSession, LinuxWriteSessionId, LinuxWritebackStore,
};
use async_trait::async_trait;
use bytes::Bytes;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn store_error(kind: CloudFilesStoreErrorKind, context: &str) -> CloudFilesStoreError {
    CloudFilesStoreError::new(kind, context)
}

fn namespace_error(
    kind: LinuxNamespaceMutationStoreErrorKind,
    context: &str,
) -> LinuxNamespaceMutationStoreError {
    LinuxNamespaceMutationStoreError::new(kind, context)
}

#[derive(Clone)]
struct FixtureBackend {
    scope: CloudScope,
    root: CloudItem,
    existing: CloudItem,
    existing_bytes: Bytes,
    content_reads: Arc<AtomicUsize>,
}

impl FixtureBackend {
    fn new() -> Self {
        let scope = CloudScope::new(
            CloudNamespaceId::new("linux-create-test").expect("namespace should be valid"),
            CloudRootId::new("root").expect("root should be valid"),
        );
        let root_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("root").expect("item should be valid"),
        );
        let existing_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("existing").expect("item should be valid"),
        );
        let root = CloudItem::directory(
            root_key.clone(),
            None,
            "root",
            MetadataRevision::from_slice(b"root-m1").expect("revision should be valid"),
        )
        .expect("root should be valid");
        let existing_bytes = Bytes::from_static(b"existing");
        let existing = CloudItem::file(
            existing_key,
            root_key.item_id().clone(),
            "file.txt",
            MetadataRevision::from_slice(b"existing-m1").expect("revision should be valid"),
            CloudContentMetadata::new(
                ContentRevision::from_slice(b"existing-c1").expect("revision should be valid"),
                None,
                8,
            ),
        )
        .expect("existing file should be valid");
        Self {
            scope,
            root,
            existing,
            existing_bytes,
            content_reads: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl CloudMetadataBackend for FixtureBackend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        if key == self.root.key() {
            Ok(self.root.clone())
        } else if key == self.existing.key() {
            Ok(self.existing.clone())
        } else {
            Err(CloudBackendError::new(CloudBackendErrorKind::NotFound))
        }
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        if cursor.is_some() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        if parent == self.root.key() {
            Ok(CloudItemPage::new(vec![self.existing.clone()], None))
        } else {
            Err(CloudBackendError::new(CloudBackendErrorKind::NotFound))
        }
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
impl CloudContentBackend for FixtureBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        self.content_reads.fetch_add(1, Ordering::SeqCst);
        let content = self
            .existing
            .content()
            .expect("existing file should have content");
        if request.key() != self.existing.key() || request.revision() != content.revision() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let (offset, bytes) = match request.read_range() {
            ContentReadRange::Whole => (0, self.existing_bytes.clone()),
            ContentReadRange::Range(range) => {
                let start = usize::try_from(range.offset())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                let end = usize::try_from(range.end_exclusive().min(8))
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                (range.offset(), self.existing_bytes.slice(start.min(8)..end))
            }
        };
        ContentReadResponse::new(content.revision().clone(), offset, bytes, 8)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

impl CloudFilesBackend for FixtureBackend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        CloudFilesCapabilities::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptanceFault {
    WrongItemKey,
    WrongParent,
    WrongSessionGeneration,
    DuplicateInode,
    FutureIntentGeneration,
}

#[derive(Clone)]
struct StagedFile {
    bytes: Vec<u8>,
    snapshot: Option<LocalContentSnapshot>,
}

struct CreateStoreState {
    active_generation: Option<SessionGeneration>,
    next_item: u64,
    next_inode: u64,
    next_session: u64,
    next_local_generation: u64,
    created: HashMap<CloudItemKey, LinuxCreatedFile>,
    names: HashMap<(CloudItemId, String), CloudItemKey>,
    files: HashMap<CloudItemKey, StagedFile>,
    sessions: HashMap<LinuxWriteSessionId, (CloudItemKey, SessionGeneration)>,
    existing_names: HashSet<(CloudItemId, String)>,
    fault: Option<AcceptanceFault>,
    fail_next_create: bool,
}

impl CreateStoreState {
    fn new(root: &CloudItemKey) -> Self {
        Self {
            active_generation: None,
            next_item: 0,
            next_inode: 100,
            next_session: 0,
            next_local_generation: 0,
            created: HashMap::new(),
            names: HashMap::new(),
            files: HashMap::new(),
            sessions: HashMap::new(),
            existing_names: HashSet::from([(root.item_id().clone(), "file.txt".to_owned())]),
            fault: None,
            fail_next_create: false,
        }
    }
}

struct MemoryCreateStore {
    state: Mutex<CreateStoreState>,
}

impl MemoryCreateStore {
    fn new(root: &CloudItemKey) -> Self {
        Self {
            state: Mutex::new(CreateStoreState::new(root)),
        }
    }

    fn configure(&self, update: impl FnOnce(&mut CreateStoreState)) {
        update(&mut lock(&self.state));
    }

    fn next_session(
        state: &mut CreateStoreState,
        key: &CloudItemKey,
        generation: SessionGeneration,
    ) -> StoreResult<LinuxWriteSession> {
        state.next_session = state.next_session.checked_add(1).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "session identity exhausted",
            )
        })?;
        let id = LinuxWriteSessionId::new(format!("create-session-{}", state.next_session))
            .map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "session identity was invalid",
                )
            })?;
        let size = state
            .files
            .get(key)
            .map(|file| u64::try_from(file.bytes.len()))
            .transpose()
            .map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "staging size exceeded u64",
                )
            })?
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "staging missing"))?;
        state.sessions.insert(id.clone(), (key.clone(), generation));
        Ok(LinuxWriteSession::new(id, key.clone(), size, generation))
    }

    fn session_key(
        state: &CreateStoreState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<CloudItemKey> {
        let (key, generation) = state
            .sessions
            .get(session)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "session missing"))?;
        if state.active_generation != Some(*generation) {
            return Err(store_error(
                CloudFilesStoreErrorKind::Conflict,
                "session was fenced",
            ));
        }
        Ok(key.clone())
    }

    fn commit(
        state: &mut CreateStoreState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<LinuxWriteCommit> {
        let key = Self::session_key(state, session)?;
        state.next_local_generation =
            state.next_local_generation.checked_add(1).ok_or_else(|| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "local generation exhausted",
                )
            })?;
        let generation =
            LocalContentGeneration::new(state.next_local_generation).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "local generation was invalid",
                )
            })?;
        let file = state
            .files
            .get_mut(&key)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "staging missing"))?;
        let snapshot = LocalContentSnapshot::new(
            key,
            generation,
            LocalContentReference::new(format!("create-snapshot-{}", generation.get())).map_err(
                |_| {
                    store_error(
                        CloudFilesStoreErrorKind::InvalidTransition,
                        "snapshot reference was invalid",
                    )
                },
            )?,
            u64::try_from(file.bytes.len()).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "staging size exceeded u64",
                )
            })?,
            None,
        );
        file.snapshot = Some(snapshot.clone());
        Ok(LinuxWriteCommit::new(snapshot))
    }
}

#[async_trait]
#[expect(
    clippy::too_many_lines,
    reason = "the in-memory namespace store keeps the complete durability contract in one test implementation"
)]
impl LinuxNamespaceMutationStore for MemoryCreateStore {
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
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "mount generation regressed",
            ));
        }
        state.active_generation = Some(session_generation);
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
        if state.active_generation != Some(request.session_generation()) {
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "create used an inactive mount generation",
            ));
        }
        if state.fail_next_create {
            state.fail_next_create = false;
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                "injected create transaction failure",
            ));
        }
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        if state.existing_names.contains(&name_key) || state.names.contains_key(&name_key) {
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
                "parent/name already exists",
            ));
        }
        state.next_item += 1;
        state.next_inode += 1;
        let stable_key = CloudItemKey::new(
            request.parent_key().scope().clone(),
            CloudItemId::new(format!("created-{}", state.next_item)).map_err(|_| {
                namespace_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "allocated item id was invalid",
                )
            })?,
        );
        let item = CloudItem::file(
            stable_key.clone(),
            request.parent_key().item_id().clone(),
            request.name(),
            MetadataRevision::from_slice(format!("local-m{}", state.next_item).as_bytes())
                .map_err(|_| {
                    namespace_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "local metadata revision was invalid",
                    )
                })?,
            CloudContentMetadata::new(
                ContentRevision::from_slice(format!("local-c{}", state.next_item).as_bytes())
                    .map_err(|_| {
                        namespace_error(
                            LinuxNamespaceMutationStoreErrorKind::Conflict,
                            "local content revision was invalid",
                        )
                    })?,
                None,
                0,
            ),
        )
        .map_err(|_| {
            namespace_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "allocated local item was invalid",
            )
        })?;
        let inode = LinuxInode::new(state.next_inode).map_err(|_| {
            namespace_error(
                LinuxNamespaceMutationStoreErrorKind::Conflict,
                "allocated inode was invalid",
            )
        })?;
        let record = LinuxInodeRecord::new(
            stable_key.clone(),
            inode,
            LinuxInodeGeneration::new(state.next_item).map_err(|_| {
                namespace_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "allocated inode generation was invalid",
                )
            })?,
        );
        let intent = MutationIntent::new(
            OperationId::new(format!("create-op-{}", state.next_item)).map_err(|_| {
                namespace_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "operation id was invalid",
                )
            })?,
            IdempotencyKey::new(format!("create-idempotency-{}", state.next_item)).map_err(
                |_| {
                    namespace_error(
                        LinuxNamespaceMutationStoreErrorKind::Conflict,
                        "idempotency key was invalid",
                    )
                },
            )?,
            MutationOrigin::PlatformCommand,
            request.session_generation(),
            DesiredMutation::create(
                request.parent_key().scope().clone(),
                request.parent_key().item_id().clone(),
                request.name(),
                aster_forge_cloud_files_core::CloudItemKind::File,
            )
            .map_err(|_| {
                namespace_error(
                    LinuxNamespaceMutationStoreErrorKind::Conflict,
                    "create mutation was invalid",
                )
            })?,
            MutationPreconditions::default(),
        );
        let durable = LinuxCreatedFile::new(item, record, intent);
        state.files.insert(
            stable_key.clone(),
            StagedFile {
                bytes: Vec::new(),
                snapshot: None,
            },
        );
        state.names.insert(name_key, stable_key.clone());
        state.created.insert(stable_key.clone(), durable.clone());
        let session = Self::next_session(&mut state, &stable_key, request.session_generation())
            .map_err(|error| {
                namespace_error(
                    LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                    error.context(),
                )
            })?;

        let mut returned = durable;
        let mut returned_session = session;
        match state.fault.take() {
            Some(AcceptanceFault::WrongItemKey) => {
                let wrong_key = CloudItemKey::new(
                    returned.item().key().scope().clone(),
                    CloudItemId::new("wrong-returned-item").expect("fault id should be valid"),
                );
                let wrong_item = CloudItem::file(
                    wrong_key,
                    request.parent_key().item_id().clone(),
                    request.name(),
                    MetadataRevision::from_slice(b"wrong-m").expect("revision should be valid"),
                    CloudContentMetadata::new(
                        ContentRevision::from_slice(b"wrong-c").expect("revision should be valid"),
                        None,
                        0,
                    ),
                )
                .expect("fault item should be valid");
                returned = LinuxCreatedFile::new(
                    wrong_item,
                    returned.inode_record().clone(),
                    returned.mutation_intent().clone(),
                );
            }
            Some(AcceptanceFault::WrongParent) => {
                let wrong_item = CloudItem::file(
                    returned.item().key().clone(),
                    CloudItemId::new("wrong-parent").expect("fault parent should be valid"),
                    request.name(),
                    MetadataRevision::from_slice(b"wrong-parent-m")
                        .expect("revision should be valid"),
                    CloudContentMetadata::new(
                        ContentRevision::from_slice(b"wrong-parent-c")
                            .expect("revision should be valid"),
                        None,
                        0,
                    ),
                )
                .expect("fault item should be valid");
                returned = LinuxCreatedFile::new(
                    wrong_item,
                    returned.inode_record().clone(),
                    returned.mutation_intent().clone(),
                );
            }
            Some(AcceptanceFault::WrongSessionGeneration) => {
                returned_session = LinuxWriteSession::new(
                    returned_session.id().clone(),
                    returned_session.key().clone(),
                    0,
                    SessionGeneration::new(request.session_generation().get() + 1)
                        .expect("fault generation should be valid"),
                );
            }
            Some(AcceptanceFault::DuplicateInode) => {
                returned = LinuxCreatedFile::new(
                    returned.item().clone(),
                    LinuxInodeRecord::new(
                        returned.item().key().clone(),
                        LinuxInode::new(2).expect("fixture inode should be valid"),
                        returned.inode_record().generation(),
                    ),
                    returned.mutation_intent().clone(),
                );
            }
            Some(AcceptanceFault::FutureIntentGeneration) => {
                let wrong_intent = MutationIntent::new(
                    returned.mutation_intent().operation_id().clone(),
                    returned.mutation_intent().idempotency_key().clone(),
                    MutationOrigin::PlatformCommand,
                    SessionGeneration::new(request.session_generation().get() + 1)
                        .expect("fault generation should be valid"),
                    returned.mutation_intent().desired().clone(),
                    MutationPreconditions::default(),
                );
                returned = LinuxCreatedFile::new(
                    returned.item().clone(),
                    returned.inode_record().clone(),
                    wrong_intent,
                );
            }
            None => {}
        }
        Ok(LinuxCreateFileAcceptance::new(returned, returned_session))
    }

    async fn open_created_file(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> LinuxNamespaceStoreResult<LinuxWriteSession> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(session_generation) {
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::Fenced,
                "created open used an inactive mount generation",
            ));
        }
        if !state.created.contains_key(key) {
            return Err(namespace_error(
                LinuxNamespaceMutationStoreErrorKind::NotFound,
                "created item was missing",
            ));
        }
        Self::next_session(&mut state, key, session_generation).map_err(|error| {
            namespace_error(
                LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
                error.context(),
            )
        })
    }
}

#[async_trait]
impl LinuxWritebackStore for MemoryCreateStore {
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
                "mount generation regressed",
            ));
        }
        state.active_generation = Some(session_generation);
        Ok(state
            .files
            .iter()
            .filter(|(key, _)| key.scope() == scope)
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
                "recovered open used a fenced generation",
            ));
        }
        let snapshot = state.files.get(key).and_then(|file| file.snapshot.clone());
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let session = Self::next_session(&mut state, key, session_generation)?;
        Ok(Some(LinuxWriteSession::from_recovered(
            session.id().clone(),
            snapshot,
            session_generation,
        )))
    }

    async fn open_existing(
        &self,
        request: &LinuxWriteOpenRequest,
        base_content: Bytes,
    ) -> StoreResult<LinuxWriteSession> {
        let mut state = lock(&self.state);
        state
            .files
            .entry(request.key().clone())
            .or_insert(StagedFile {
                bytes: base_content.to_vec(),
                snapshot: None,
            });
        Self::next_session(&mut state, request.key(), request.session_generation())
    }

    async fn read(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        size: u32,
    ) -> StoreResult<Bytes> {
        let state = lock(&self.state);
        let key = Self::session_key(&state, session)?;
        let bytes = &state
            .files
            .get(&key)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "staging missing"))?
            .bytes;
        let start = usize::try_from(offset)
            .unwrap_or(bytes.len())
            .min(bytes.len());
        let end = start.saturating_add(size as usize).min(bytes.len());
        Ok(Bytes::copy_from_slice(&bytes[start..end]))
    }

    async fn write(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        bytes: Bytes,
    ) -> StoreResult<LinuxWriteCommit> {
        let mut state = lock(&self.state);
        let key = Self::session_key(&state, session)?;
        let file = state
            .files
            .get_mut(&key)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "staging missing"))?;
        let start = usize::try_from(offset).map_err(|_| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "offset exceeded usize",
            )
        })?;
        let end = start.checked_add(bytes.len()).ok_or_else(|| {
            store_error(
                CloudFilesStoreErrorKind::InvalidTransition,
                "write end overflowed",
            )
        })?;
        if file.bytes.len() < end {
            file.bytes.resize(end, 0);
        }
        file.bytes[start..end].copy_from_slice(&bytes);
        Self::commit(&mut state, session)
    }

    async fn truncate(
        &self,
        session: &LinuxWriteSessionId,
        size: u64,
    ) -> StoreResult<LinuxWriteCommit> {
        let mut state = lock(&self.state);
        let key = Self::session_key(&state, session)?;
        let file = state
            .files
            .get_mut(&key)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "staging missing"))?;
        file.bytes.resize(
            usize::try_from(size).map_err(|_| {
                store_error(
                    CloudFilesStoreErrorKind::InvalidTransition,
                    "truncate size exceeded usize",
                )
            })?,
            0,
        );
        Self::commit(&mut state, session)
    }

    async fn sync(
        &self,
        session: &LinuxWriteSessionId,
        _data_only: bool,
    ) -> StoreResult<Option<LinuxWriteCommit>> {
        let state = lock(&self.state);
        let key = Self::session_key(&state, session)?;
        Ok(state
            .files
            .get(&key)
            .and_then(|file| file.snapshot.clone())
            .map(LinuxWriteCommit::new))
    }

    async fn close(&self, session: &LinuxWriteSessionId) -> StoreResult<()> {
        lock(&self.state)
            .sessions
            .remove(session)
            .ok_or_else(|| store_error(CloudFilesStoreErrorKind::NotFound, "session missing"))?;
        Ok(())
    }
}

fn fixture_readonly(backend: FixtureBackend) -> (LinuxReadOnlyEngine<FixtureBackend>, LinuxInode) {
    let root = LinuxInodeRecord::new(
        backend.root.key().clone(),
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    let existing_inode = LinuxInode::new(2).expect("inode should be valid");
    let existing = LinuxInodeRecord::new(
        backend.existing.key().clone(),
        existing_inode,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    (
        LinuxReadOnlyEngine::new(
            Arc::new(backend),
            Arc::new(LinuxInodeTable::new(root, [existing]).expect("inode table should be valid")),
            LinuxAttributePolicy::new(1000, 1000, 0o644, 0o755, Duration::ZERO)
                .expect("attribute policy should be valid"),
        ),
        existing_inode,
    )
}

async fn activate(
    backend: FixtureBackend,
    store: Arc<MemoryCreateStore>,
    generation: u64,
) -> LinuxWritableEngine<FixtureBackend, MemoryCreateStore> {
    let (readonly, _) = fixture_readonly(backend);
    LinuxWritableEngine::activate_with_namespace(
        readonly,
        store.clone(),
        store,
        SessionGeneration::new(generation).expect("generation should be valid"),
    )
    .await
    .expect("creatable mount should activate")
}

#[tokio::test]
async fn create_returns_stable_entry_open_handle_and_durable_empty_staging() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend.clone(), store.clone(), 1).await;
    let (node, handle) = engine
        .create_file(
            LINUX_ROOT_INODE,
            "new.txt",
            0o100_664,
            0o002,
            LinuxFileAccess::ReadWrite,
        )
        .await
        .expect("create should succeed");
    assert_eq!(node.key().item_id().as_str(), "created-1");
    assert_eq!(node.attributes().inode().get(), 101);
    assert_eq!(node.generation().get(), 1);
    assert_eq!(node.attributes().size(), 0);
    assert!(
        engine
            .read_file(node.attributes().inode(), handle, 0, 8)
            .await
            .expect("empty staging should read")
            .is_empty()
    );
    assert_eq!(backend.content_reads.load(Ordering::SeqCst), 0);
    let state = lock(&store.state);
    let created = state
        .created
        .get(node.key())
        .expect("create record should be durable");
    assert!(matches!(
        created.mutation_intent().desired(),
        DesiredMutation::Create { name, .. } if name == "new.txt"
    ));
    assert_eq!(created.inode_record().inode(), node.attributes().inode());
}

#[tokio::test]
async fn create_write_read_sync_release_reopen_and_directory_snapshot_work() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend, store, 1).await;
    let before = engine
        .open_directory(LINUX_ROOT_INODE)
        .await
        .expect("directory should open");
    let (node, handle) = engine
        .create_file(
            LINUX_ROOT_INODE,
            "written.txt",
            0o100_644,
            0,
            LinuxFileAccess::ReadWrite,
        )
        .await
        .expect("create should succeed");
    assert!(
        engine
            .directory_snapshot(LINUX_ROOT_INODE, before)
            .expect("old snapshot should remain valid")
            .entries()
            .iter()
            .all(|entry| entry.name() != "written.txt")
    );
    engine
        .write_file(
            node.attributes().inode(),
            handle,
            0,
            Bytes::from_static(b"new file\n"),
        )
        .await
        .expect("write should persist");
    engine
        .sync_file(node.attributes().inode(), handle, false)
        .await
        .expect("sync should succeed");
    engine
        .release_file(handle)
        .await
        .expect("create handle should release");
    let reopened = engine
        .open_file(node.attributes().inode(), LinuxFileAccess::Read)
        .await
        .expect("created file should reopen");
    assert_eq!(
        engine
            .read_file(node.attributes().inode(), reopened, 0, 64)
            .await
            .expect("reopened bytes should read"),
        Bytes::from_static(b"new file\n")
    );
    let after = engine
        .open_directory(LINUX_ROOT_INODE)
        .await
        .expect("new directory snapshot should open");
    assert!(
        engine
            .directory_snapshot(LINUX_ROOT_INODE, after)
            .expect("new snapshot should remain valid")
            .entries()
            .iter()
            .any(|entry| entry.name() == "written.txt")
    );
    engine
        .release_directory(before)
        .expect("old directory should release");
    engine
        .release_directory(after)
        .expect("new directory should release");
}

#[tokio::test]
async fn existing_and_concurrent_same_name_create_report_one_conflict() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend, store, 1).await;
    assert!(matches!(
        engine
            .create_file(
                LINUX_ROOT_INODE,
                "file.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::NamespaceStore(error))
            if error.kind() == LinuxNamespaceMutationStoreErrorKind::AlreadyExists
    ));
    let (left, right) = tokio::join!(
        engine.create_file(
            LINUX_ROOT_INODE,
            "race.txt",
            0o100_644,
            0,
            LinuxFileAccess::Write,
        ),
        engine.create_file(
            LINUX_ROOT_INODE,
            "race.txt",
            0o100_644,
            0,
            LinuxFileAccess::Write,
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let error = if let Err(error) = left {
        error
    } else {
        right.expect_err("the other racing create should fail")
    };
    assert!(matches!(
        error,
        LinuxCloudFilesError::NamespaceStore(error)
            if error.kind() == LinuxNamespaceMutationStoreErrorKind::AlreadyExists
    ));
}

#[tokio::test]
async fn concurrent_different_name_creates_keep_distinct_stable_allocations() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend, store, 1).await;
    let (left, right) = tokio::join!(
        engine.create_file(
            LINUX_ROOT_INODE,
            "left.txt",
            0o100_644,
            0,
            LinuxFileAccess::Write,
        ),
        engine.create_file(
            LINUX_ROOT_INODE,
            "right.txt",
            0o100_644,
            0,
            LinuxFileAccess::Write,
        )
    );
    let left = left.expect("left create should succeed").0;
    let right = right.expect("right create should succeed").0;
    assert_ne!(left.key(), right.key());
    assert_ne!(left.attributes().inode(), right.attributes().inode());
    assert_ne!(left.generation(), right.generation());
}

#[tokio::test]
async fn invalid_name_parent_and_unconfigured_namespace_fail_before_exposure() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend.clone(), store.clone(), 1).await;
    for name in ["", ".", "..", "a/b", "nul\0name"] {
        assert!(matches!(
            engine
                .create_file(LINUX_ROOT_INODE, name, 0o100_644, 0, LinuxFileAccess::Write,)
                .await,
            Err(LinuxCloudFilesError::InvalidName { .. })
        ));
    }
    assert!(matches!(
        engine
            .create_file(
                LinuxInode::new(2).expect("inode should be valid"),
                "child.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::NotDirectory)
    ));
    assert!(matches!(
        engine
            .create_file(
                LinuxInode::new(999).expect("inode should be valid"),
                "child.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::UnknownInode { inode: 999 })
    ));

    let (readonly, _) = fixture_readonly(backend);
    let plain = LinuxWritableEngine::activate(
        readonly,
        store,
        SessionGeneration::new(2).expect("generation should be valid"),
    )
    .await
    .expect("plain writable engine should activate");
    assert!(matches!(
        plain
            .create_file(
                LINUX_ROOT_INODE,
                "unsupported.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::NamespaceMutationNotConfigured)
    ));
}

#[tokio::test]
async fn store_failure_does_not_expose_an_inode_or_directory_entry() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let engine = activate(backend, store.clone(), 1).await;
    store.configure(|state| state.fail_next_create = true);
    assert!(matches!(
        engine
            .create_file(
                LINUX_ROOT_INODE,
                "failed.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::NamespaceStore(error))
            if error.kind() == LinuxNamespaceMutationStoreErrorKind::PersistenceFailure
    ));
    assert!(engine.lookup(LINUX_ROOT_INODE, "failed.txt").await.is_err());
    assert!(lock(&store.state).created.is_empty());
}

#[tokio::test]
async fn invalid_product_acceptance_is_rejected_for_every_identity_boundary() {
    for fault in [
        AcceptanceFault::WrongItemKey,
        AcceptanceFault::WrongParent,
        AcceptanceFault::WrongSessionGeneration,
        AcceptanceFault::DuplicateInode,
        AcceptanceFault::FutureIntentGeneration,
    ] {
        let backend = FixtureBackend::new();
        let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
        let engine = activate(backend, store.clone(), 1).await;
        store.configure(|state| state.fault = Some(fault));
        assert!(matches!(
            engine
                .create_file(
                    LINUX_ROOT_INODE,
                    "invalid.txt",
                    0o100_644,
                    0,
                    LinuxFileAccess::Write,
                )
                .await,
            Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
        ));
        assert!(
            engine
                .lookup(LINUX_ROOT_INODE, "invalid.txt")
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn durable_acceptance_survives_lost_reply_and_restart_without_remote_hydration() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let first = activate(backend.clone(), store.clone(), 1).await;
    let request = LinuxCreateFileRequest::new(
        backend.root.key().clone(),
        "lost-reply.txt",
        0o100_644,
        0,
        LinuxFileAccess::ReadWrite,
        first.session_generation(),
    )
    .expect("request should be valid");
    let accepted = store
        .create_file(&request)
        .await
        .expect("product transaction should commit");
    let expected_key = accepted.created().item().key().clone();
    let expected_inode = accepted.created().inode_record().inode();
    drop(first);

    let second = activate(backend.clone(), store, 2).await;
    let recovered = second
        .lookup(LINUX_ROOT_INODE, "lost-reply.txt")
        .await
        .expect("restart should recover the durable create");
    assert_eq!(recovered.key(), &expected_key);
    assert_eq!(recovered.attributes().inode(), expected_inode);
    let handle = second
        .open_file(expected_inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("recovered empty staging should open");
    assert!(
        second
            .read_file(expected_inode, handle, 0, 1)
            .await
            .expect("recovered empty staging should read")
            .is_empty()
    );
    assert_eq!(backend.content_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn created_dirty_bytes_and_size_survive_a_new_mount_generation() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let first = activate(backend.clone(), store.clone(), 1).await;
    let (node, handle) = first
        .create_file(
            LINUX_ROOT_INODE,
            "restart-dirty.txt",
            0o100_644,
            0,
            LinuxFileAccess::ReadWrite,
        )
        .await
        .expect("create should succeed");
    first
        .write_file(
            node.attributes().inode(),
            handle,
            0,
            Bytes::from_static(b"durable create bytes"),
        )
        .await
        .expect("created bytes should persist");
    first
        .release_file(handle)
        .await
        .expect("first handle should release");
    drop(first);

    let second = activate(backend.clone(), store, 2).await;
    let recovered = second
        .lookup(LINUX_ROOT_INODE, "restart-dirty.txt")
        .await
        .expect("created namespace record should recover");
    assert_eq!(recovered.key(), node.key());
    assert_eq!(recovered.attributes().inode(), node.attributes().inode());
    assert_eq!(recovered.generation(), node.generation());
    assert_eq!(recovered.attributes().size(), 20);
    let reopened = second
        .open_file(recovered.attributes().inode(), LinuxFileAccess::Read)
        .await
        .expect("dirty created file should reopen");
    assert_eq!(
        second
            .read_file(recovered.attributes().inode(), reopened, 0, 64)
            .await
            .expect("dirty created bytes should read"),
        Bytes::from_static(b"durable create bytes")
    );
    assert_eq!(backend.content_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn newer_mount_generation_fences_late_create_and_preserves_recovery() {
    let backend = FixtureBackend::new();
    let store = Arc::new(MemoryCreateStore::new(backend.root.key()));
    let first = activate(backend.clone(), store.clone(), 1).await;
    let second = activate(backend, store, 2).await;
    assert!(matches!(
        first
            .create_file(
                LINUX_ROOT_INODE,
                "stale.txt",
                0o100_644,
                0,
                LinuxFileAccess::Write,
            )
            .await,
        Err(LinuxCloudFilesError::NamespaceStore(error))
            if error.kind() == LinuxNamespaceMutationStoreErrorKind::Fenced
    ));
    second
        .create_file(
            LINUX_ROOT_INODE,
            "current.txt",
            0o100_644,
            0,
            LinuxFileAccess::Write,
        )
        .await
        .expect("current mount should create");
}

#[test]
fn namespace_request_and_error_values_preserve_exact_boundaries() {
    let backend = FixtureBackend::new();
    let request = LinuxCreateFileRequest::new(
        backend.root.key().clone(),
        "exact.txt",
        u32::MAX,
        0o077,
        LinuxFileAccess::ReadWrite,
        SessionGeneration::new(u64::MAX).expect("generation should be valid"),
    )
    .expect("request should be valid");
    assert_eq!(request.parent_key(), backend.root.key());
    assert_eq!(request.name(), "exact.txt");
    assert_eq!(request.mode(), u32::MAX);
    assert_eq!(request.umask(), 0o077);
    assert_eq!(request.access(), LinuxFileAccess::ReadWrite);
    assert_eq!(request.session_generation().get(), u64::MAX);

    for kind in [
        LinuxNamespaceMutationStoreErrorKind::NotFound,
        LinuxNamespaceMutationStoreErrorKind::AlreadyExists,
        LinuxNamespaceMutationStoreErrorKind::Fenced,
        LinuxNamespaceMutationStoreErrorKind::Conflict,
        LinuxNamespaceMutationStoreErrorKind::PersistenceFailure,
    ] {
        let error = LinuxNamespaceMutationStoreError::new(kind, "exact context");
        assert_eq!(error.kind(), kind);
        assert_eq!(error.context(), "exact context");
    }
}
