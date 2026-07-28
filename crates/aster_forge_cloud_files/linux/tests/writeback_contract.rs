use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use aster_forge_cloud_files_core::{
    BackendResult, ChangeCursor, ChangePage, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudContentMetadata, CloudFilesBackend, CloudFilesCapabilities,
    CloudItem, CloudItemId, CloudItemKey, CloudItemPage, CloudMetadataBackend, CloudNamespaceId,
    CloudRootId, CloudScope, ContentReadRange, ContentReadRequest, ContentReadResponse,
    ContentRevision, LocalContentGeneration, LocalContentReference, LocalContentSnapshot,
    MetadataRevision, PageCursor, SessionGeneration, StoreResult,
};
use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxAttributePolicy, LinuxCloudFilesError, LinuxFileAccess, LinuxInode,
    LinuxInodeGeneration, LinuxInodeRecord, LinuxInodeTable, LinuxReadOnlyEngine,
    LinuxWritableEngine, LinuxWriteCommit, LinuxWriteOpenRequest, LinuxWriteSession,
    LinuxWriteSessionId, LinuxWritebackStore,
};
use async_trait::async_trait;
use bytes::Bytes;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Clone)]
struct FixtureBackend {
    scope: CloudScope,
    root: CloudItem,
    file: CloudItem,
    bytes: Bytes,
    content_reads: Arc<AtomicUsize>,
}

impl FixtureBackend {
    fn new() -> Self {
        let scope = CloudScope::new(
            CloudNamespaceId::new("linux-write-test").expect("namespace should be valid"),
            CloudRootId::new("root").expect("root id should be valid"),
        );
        let root_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("root").expect("item id should be valid"),
        );
        let file_key = CloudItemKey::new(
            scope.clone(),
            CloudItemId::new("file").expect("item id should be valid"),
        );
        let bytes = Bytes::from_static(b"abcdef");
        let root = CloudItem::directory(
            root_key.clone(),
            None,
            "root",
            MetadataRevision::from_slice(b"root-m1").expect("revision should be valid"),
        )
        .expect("root should be valid");
        let file = CloudItem::file(
            file_key,
            root_key.item_id().clone(),
            "file.txt",
            MetadataRevision::from_slice(b"file-m1").expect("revision should be valid"),
            CloudContentMetadata::new(
                ContentRevision::from_slice(b"file-c1").expect("revision should be valid"),
                None,
                u64::try_from(bytes.len()).expect("fixture length should fit u64"),
            ),
        )
        .expect("file should be valid");
        Self {
            scope,
            root,
            file,
            bytes,
            content_reads: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn content_reads(&self) -> usize {
        self.content_reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CloudMetadataBackend for FixtureBackend {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        if key == self.root.key() {
            Ok(self.root.clone())
        } else if key == self.file.key() {
            Ok(self.file.clone())
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
            Ok(CloudItemPage::new(vec![self.file.clone()], None))
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
            .file
            .content()
            .expect("fixture file should have content");
        if request.key() != self.file.key() || request.revision() != content.revision() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let total = u64::try_from(self.bytes.len())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let (offset, bytes) = match request.read_range() {
            ContentReadRange::Whole => (0, self.bytes.clone()),
            ContentReadRange::Range(range) => {
                let start = usize::try_from(range.offset())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                let end = usize::try_from(range.end_exclusive().min(total))
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                (
                    range.offset(),
                    self.bytes.slice(start.min(self.bytes.len())..end),
                )
            }
        };
        ContentReadResponse::new(content.revision().clone(), offset, bytes, total)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

impl CloudFilesBackend for FixtureBackend {
    fn capabilities(&self) -> CloudFilesCapabilities {
        CloudFilesCapabilities::default()
    }
}

#[derive(Default)]
struct MemoryWritebackState {
    active_generation: Option<SessionGeneration>,
    next_session: u64,
    next_generation: u64,
    sessions: HashMap<LinuxWriteSessionId, (CloudItemKey, Vec<u8>, Option<LocalContentSnapshot>)>,
    session_generations: HashMap<LinuxWriteSessionId, SessionGeneration>,
    durable: HashMap<CloudItemKey, (Vec<u8>, LocalContentSnapshot)>,
    closed: Vec<LinuxWriteSessionId>,
    partial_reads: bool,
    wrong_open_key: bool,
    wrong_commit_key: bool,
    wrong_recovered_session_generation: bool,
    recovery_override: Option<Vec<LocalContentSnapshot>>,
    conflicting_sync_snapshot: bool,
    regress_generation: bool,
    defer_first_commit: bool,
}

#[derive(Default)]
struct MemoryWritebackStore {
    state: Mutex<MemoryWritebackState>,
    first_commit_ready: tokio::sync::Notify,
    release_first_commit: tokio::sync::Notify,
}

impl MemoryWritebackStore {
    fn configure(&self, update: impl FnOnce(&mut MemoryWritebackState)) {
        update(&mut lock(&self.state));
    }

    fn commit(
        state: &mut MemoryWritebackState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<LinuxWriteCommit> {
        let regress = state.regress_generation;
        let wrong_key = state.wrong_commit_key;
        if !regress {
            state.next_generation += 1;
        }
        let generation = if regress { 1 } else { state.next_generation };
        let (key, bytes, snapshot) = {
            let (key, bytes, latest) = state.sessions.get_mut(session).ok_or_else(|| {
                aster_forge_cloud_files_core::CloudFilesStoreError::new(
                    aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                    "missing write session",
                )
            })?;
            let snapshot_key = if wrong_key {
                CloudItemKey::new(
                    key.scope().clone(),
                    CloudItemId::new("wrong-item").expect("fixture id should be valid"),
                )
            } else {
                key.clone()
            };
            let snapshot = LocalContentSnapshot::new(
                snapshot_key,
                LocalContentGeneration::new(generation).expect("generation should be non-zero"),
                LocalContentReference::new(format!("memory-write-{generation}"))
                    .expect("reference should be valid"),
                u64::try_from(bytes.len()).expect("fixture length should fit u64"),
                None,
            );
            *latest = Some(snapshot.clone());
            (key.clone(), bytes.clone(), snapshot)
        };
        state.durable.insert(key, (bytes, snapshot.clone()));
        Ok(LinuxWriteCommit::new(snapshot))
    }

    fn active_generation(
        state: &MemoryWritebackState,
        session: &LinuxWriteSessionId,
    ) -> StoreResult<()> {
        let generation = state.session_generations.get(session).ok_or_else(|| {
            aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                "missing write session generation",
            )
        })?;
        if state.active_generation != Some(*generation) {
            return Err(aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::Conflict,
                "write session was fenced by a newer mount generation",
            ));
        }
        Ok(())
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
            return Err(aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::Conflict,
                "mount generation regressed",
            ));
        }
        state.active_generation = Some(session_generation);
        if let Some(snapshots) = state.recovery_override.clone() {
            return Ok(snapshots);
        }
        Ok(state
            .durable
            .iter()
            .filter(|(key, _)| key.scope() == scope)
            .map(|(_, (_, snapshot))| snapshot.clone())
            .collect())
    }

    async fn open_recovered(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> StoreResult<Option<LinuxWriteSession>> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(session_generation) {
            return Err(aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::Conflict,
                "recovered open used a fenced mount generation",
            ));
        }
        let Some((bytes, snapshot)) = state.durable.get(key).cloned() else {
            return Ok(None);
        };
        state.next_session += 1;
        let id = LinuxWriteSessionId::new(format!("session-{}", state.next_session))
            .expect("session should be valid");
        state
            .sessions
            .insert(id.clone(), (key.clone(), bytes, Some(snapshot.clone())));
        state
            .session_generations
            .insert(id.clone(), session_generation);
        let returned_generation = if state.wrong_recovered_session_generation {
            SessionGeneration::new(session_generation.get() + 1)
                .expect("fixture generation should be valid")
        } else {
            session_generation
        };
        Ok(Some(LinuxWriteSession::from_recovered(
            id,
            snapshot,
            returned_generation,
        )))
    }

    async fn open_existing(
        &self,
        request: &LinuxWriteOpenRequest,
        base_content: Bytes,
    ) -> StoreResult<LinuxWriteSession> {
        let mut state = lock(&self.state);
        if state.active_generation != Some(request.session_generation()) {
            return Err(aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::Conflict,
                "fresh open used a fenced mount generation",
            ));
        }
        state.next_session += 1;
        let id = LinuxWriteSessionId::new(format!("session-{}", state.next_session))
            .expect("session should be valid");
        let key = if state.wrong_open_key {
            CloudItemKey::new(
                request.key().scope().clone(),
                CloudItemId::new("wrong-open").expect("fixture id should be valid"),
            )
        } else {
            request.key().clone()
        };
        state.sessions.insert(
            id.clone(),
            (request.key().clone(), base_content.to_vec(), None),
        );
        state
            .session_generations
            .insert(id.clone(), request.session_generation());
        Ok(LinuxWriteSession::new(
            id,
            key,
            request.base_size(),
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
        let (_, bytes, _) = state.sessions.get(session).ok_or_else(|| {
            aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                "missing write session",
            )
        })?;
        let start = usize::try_from(offset)
            .unwrap_or(bytes.len())
            .min(bytes.len());
        let mut end = start.saturating_add(size as usize).min(bytes.len());
        if state.partial_reads && end > start {
            end -= 1;
        }
        Ok(Bytes::copy_from_slice(&bytes[start..end]))
    }

    async fn write(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        bytes: Bytes,
    ) -> StoreResult<LinuxWriteCommit> {
        let (commit, defer) = {
            let mut state = lock(&self.state);
            Self::active_generation(&state, session)?;
            let (_, target, _) = state.sessions.get_mut(session).ok_or_else(|| {
                aster_forge_cloud_files_core::CloudFilesStoreError::new(
                    aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                    "missing write session",
                )
            })?;
            let start = usize::try_from(offset).map_err(|_| {
                aster_forge_cloud_files_core::CloudFilesStoreError::new(
                    aster_forge_cloud_files_core::CloudFilesStoreErrorKind::InvalidTransition,
                    "offset does not fit memory fixture",
                )
            })?;
            let end = start.checked_add(bytes.len()).ok_or_else(|| {
                aster_forge_cloud_files_core::CloudFilesStoreError::new(
                    aster_forge_cloud_files_core::CloudFilesStoreErrorKind::InvalidTransition,
                    "write end overflow",
                )
            })?;
            if target.len() < end {
                target.resize(end, 0);
            }
            target[start..end].copy_from_slice(&bytes);
            let commit = Self::commit(&mut state, session)?;
            let defer = state.defer_first_commit && commit.snapshot().generation().get() == 1;
            (commit, defer)
        };
        if defer {
            self.first_commit_ready.notify_one();
            self.release_first_commit.notified().await;
        }
        Ok(commit)
    }

    async fn truncate(
        &self,
        session: &LinuxWriteSessionId,
        size: u64,
    ) -> StoreResult<LinuxWriteCommit> {
        let mut state = lock(&self.state);
        Self::active_generation(&state, session)?;
        let (_, target, _) = state.sessions.get_mut(session).ok_or_else(|| {
            aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                "missing write session",
            )
        })?;
        let size = usize::try_from(size).map_err(|_| {
            aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::InvalidTransition,
                "size does not fit memory fixture",
            )
        })?;
        target.resize(size, 0);
        Self::commit(&mut state, session)
    }

    async fn sync(
        &self,
        session: &LinuxWriteSessionId,
        _data_only: bool,
    ) -> StoreResult<Option<LinuxWriteCommit>> {
        let state = lock(&self.state);
        Self::active_generation(&state, session)?;
        let latest = state
            .sessions
            .get(session)
            .ok_or_else(|| {
                aster_forge_cloud_files_core::CloudFilesStoreError::new(
                    aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                    "missing write session",
                )
            })?
            .2
            .clone();
        let latest = if state.conflicting_sync_snapshot {
            latest.map(|snapshot| {
                LocalContentSnapshot::new(
                    snapshot.item_key().clone(),
                    snapshot.generation(),
                    LocalContentReference::new("conflicting-sync-reference")
                        .expect("reference should be valid"),
                    snapshot.size(),
                    None,
                )
            })
        } else {
            latest
        };
        Ok(latest.map(LinuxWriteCommit::new))
    }

    async fn close(&self, session: &LinuxWriteSessionId) -> StoreResult<()> {
        let mut state = lock(&self.state);
        state.sessions.remove(session).ok_or_else(|| {
            aster_forge_cloud_files_core::CloudFilesStoreError::new(
                aster_forge_cloud_files_core::CloudFilesStoreErrorKind::NotFound,
                "missing write session",
            )
        })?;
        state.session_generations.remove(session);
        state.closed.push(session.clone());
        Ok(())
    }
}

fn fixture_readonly(backend: FixtureBackend) -> (LinuxReadOnlyEngine<FixtureBackend>, LinuxInode) {
    let root = LinuxInodeRecord::new(
        backend.root.key().clone(),
        LINUX_ROOT_INODE,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    let file_inode = LinuxInode::new(2).expect("inode should be valid");
    let file = LinuxInodeRecord::new(
        backend.file.key().clone(),
        file_inode,
        LinuxInodeGeneration::new(1).expect("generation should be valid"),
    );
    (
        LinuxReadOnlyEngine::new(
            std::sync::Arc::new(backend),
            std::sync::Arc::new(
                LinuxInodeTable::new(root, [file]).expect("inode table should be valid"),
            ),
            LinuxAttributePolicy::new(1000, 1000, 0o644, 0o755, Duration::ZERO)
                .expect("attribute policy should be valid"),
        ),
        file_inode,
    )
}

async fn fixture_engine() -> (
    LinuxWritableEngine<FixtureBackend, MemoryWritebackStore>,
    std::sync::Arc<MemoryWritebackStore>,
    LinuxInode,
) {
    let (readonly, file_inode) = fixture_readonly(FixtureBackend::new());
    let store = std::sync::Arc::new(MemoryWritebackStore::default());
    (
        LinuxWritableEngine::activate(
            readonly,
            store.clone(),
            SessionGeneration::new(1).expect("session generation should be valid"),
        )
        .await
        .expect("mount activation should succeed"),
        store,
        file_inode,
    )
}

fn local_snapshot(key: CloudItemKey, generation: u64, size: u64) -> LocalContentSnapshot {
    LocalContentSnapshot::new(
        key,
        LocalContentGeneration::new(generation).expect("generation should be valid"),
        LocalContentReference::new(format!("recovery-{generation}"))
            .expect("reference should be valid"),
        size,
        None,
    )
}

#[tokio::test]
async fn writeback_hydrates_reads_writes_extends_truncates_syncs_and_releases() {
    let (engine, store, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("writable file should open");
    assert_eq!(
        engine
            .read_file(inode, handle, 0, 6)
            .await
            .expect("base bytes should read"),
        Bytes::from_static(b"abcdef")
    );
    assert_eq!(
        engine
            .write_file(inode, handle, 2, Bytes::from_static(b"XY"))
            .await
            .expect("positioned write should persist"),
        2
    );
    assert_eq!(
        engine
            .read_file(inode, handle, 0, 6)
            .await
            .expect("staged bytes should be visible"),
        Bytes::from_static(b"abXYef")
    );
    engine
        .write_file(inode, handle, 8, Bytes::from_static(b"z"))
        .await
        .expect("sparse extension should persist");
    assert_eq!(
        engine
            .read_file(inode, handle, 6, 3)
            .await
            .expect("extended bytes should read"),
        Bytes::from_static(&[0, 0, b'z'])
    );
    assert_eq!(
        engine
            .getattr(inode, Some(handle))
            .await
            .expect("staged attributes should load")
            .attributes()
            .size(),
        9
    );
    let node = engine
        .truncate_file(inode, handle, 4)
        .await
        .expect("truncate should persist");
    assert_eq!(node.attributes().size(), 4);
    assert_eq!(
        engine
            .read_file(inode, handle, 0, 8)
            .await
            .expect("truncated bytes should read"),
        Bytes::from_static(b"abXY")
    );
    engine
        .sync_file(inode, handle, false)
        .await
        .expect("flush-style sync should be idempotent");
    engine
        .sync_file(inode, handle, true)
        .await
        .expect("data-only fsync should be idempotent");
    engine
        .release_file(handle)
        .await
        .expect("release should close staging");
    assert_eq!(lock(&store.state).closed.len(), 1);
    assert!(matches!(
        engine.read_file(inode, handle, 0, 1).await,
        Err(LinuxCloudFilesError::StaleHandle)
    ));
}

#[tokio::test]
async fn access_modes_and_handle_identity_are_enforced() {
    let (engine, _, inode) = fixture_engine().await;
    let read = engine
        .open_file(inode, LinuxFileAccess::Read)
        .await
        .expect("read handle should open");
    assert!(matches!(
        engine
            .write_file(inode, read, 0, Bytes::from_static(b"x"))
            .await,
        Err(LinuxCloudFilesError::AccessModeMismatch)
    ));
    assert_eq!(
        engine
            .read_file(inode, read, 1, 2)
            .await
            .expect("remote read should work"),
        Bytes::from_static(b"bc")
    );
    engine
        .sync_file(inode, read, false)
        .await
        .expect("read-only flush should be an idempotent no-op");
    engine
        .sync_file(inode, read, true)
        .await
        .expect("read-only fsync should be an idempotent no-op");
    engine
        .release_file(read)
        .await
        .expect("remote handle should release");

    let write = engine
        .open_file(inode, LinuxFileAccess::Write)
        .await
        .expect("write handle should open");
    assert!(matches!(
        engine.read_file(inode, write, 0, 1).await,
        Err(LinuxCloudFilesError::AccessModeMismatch)
    ));
    assert!(matches!(
        engine
            .write_file(
                LinuxInode::new(3).expect("inode should be valid"),
                write,
                0,
                Bytes::from_static(b"x")
            )
            .await,
        Err(LinuxCloudFilesError::AccessModeMismatch) | Err(LinuxCloudFilesError::StaleHandle)
    ));
    engine
        .release_file(write)
        .await
        .expect("write handle should release");
}

#[tokio::test]
async fn writeback_rejects_store_identity_partial_read_and_generation_regression() {
    let (engine, store, inode) = fixture_engine().await;
    store.configure(|state| state.wrong_open_key = true);
    assert!(matches!(
        engine.open_file(inode, LinuxFileAccess::Write).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));

    let (engine, store, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("write handle should open");
    store.configure(|state| state.partial_reads = true);
    assert!(matches!(
        engine.read_file(inode, handle, 0, 2).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));
    store.configure(|state| {
        state.partial_reads = false;
        state.wrong_commit_key = true;
    });
    assert!(matches!(
        engine
            .write_file(inode, handle, 0, Bytes::from_static(b"x"))
            .await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));

    let (engine, store, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::Write)
        .await
        .expect("write handle should open");
    engine
        .write_file(inode, handle, 0, Bytes::from_static(b"x"))
        .await
        .expect("first generation should commit");
    store.configure(|state| state.regress_generation = true);
    assert!(matches!(
        engine
            .write_file(inode, handle, 1, Bytes::from_static(b"y"))
            .await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));

    let (engine, store, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::Write)
        .await
        .expect("write handle should open");
    engine
        .write_file(inode, handle, 0, Bytes::from_static(b"x"))
        .await
        .expect("first generation should commit");
    store.configure(|state| state.conflicting_sync_snapshot = true);
    assert!(matches!(
        engine.sync_file(inode, handle, false).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));
}

#[tokio::test]
async fn empty_and_overflowing_writes_have_deterministic_boundaries() {
    let (engine, _, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::Write)
        .await
        .expect("write handle should open");
    assert_eq!(
        engine
            .write_file(inode, handle, u64::MAX, Bytes::new())
            .await
            .expect("empty writes should be no-ops"),
        0
    );
    assert!(matches!(
        engine
            .write_file(inode, handle, u64::MAX, Bytes::from_static(b"x"))
            .await,
        Err(LinuxCloudFilesError::InvalidConfiguration { .. })
    ));
}

#[tokio::test]
async fn late_older_generation_is_fenced_after_a_newer_concurrent_commit() {
    let (engine, store, inode) = fixture_engine().await;
    let handle = engine
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("write handle should open");
    store.configure(|state| state.defer_first_commit = true);

    let ready = store.first_commit_ready.notified();
    let first_engine = engine.clone();
    let first = tokio::spawn(async move {
        first_engine
            .write_file(inode, handle, 0, Bytes::from_static(b"x"))
            .await
    });
    ready.await;

    assert_eq!(
        engine
            .write_file(inode, handle, 1, Bytes::from_static(b"y"))
            .await
            .expect("newer generation should apply first"),
        1
    );
    store.release_first_commit.notify_one();
    assert!(matches!(
        first.await.expect("first write task should join"),
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));
    assert_eq!(
        engine
            .read_file(inode, handle, 0, 2)
            .await
            .expect("durably staged bytes should remain readable"),
        Bytes::from_static(b"xy")
    );
}

#[tokio::test]
async fn restart_restores_dirty_size_and_reopens_without_remote_content_hydration() {
    let store = Arc::new(MemoryWritebackStore::default());
    let first_backend = FixtureBackend::new();
    let first_probe = first_backend.clone();
    let (first_readonly, inode) = fixture_readonly(first_backend);
    let first = LinuxWritableEngine::activate(
        first_readonly,
        store.clone(),
        SessionGeneration::new(1).expect("generation should be valid"),
    )
    .await
    .expect("first mount should activate");
    let first_handle = first
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("base file should hydrate");
    assert_eq!(first_probe.content_reads(), 1);
    first
        .write_file(inode, first_handle, 6, Bytes::from_static(b"XYZ"))
        .await
        .expect("dirty generation should persist");
    first
        .release_file(first_handle)
        .await
        .expect("first handle should release");

    let second_backend = FixtureBackend::new();
    let second_probe = second_backend.clone();
    let (second_readonly, second_inode) = fixture_readonly(second_backend);
    let second = LinuxWritableEngine::activate(
        second_readonly,
        store,
        SessionGeneration::new(2).expect("generation should be valid"),
    )
    .await
    .expect("restart mount should recover");
    assert_eq!(inode, second_inode);
    assert_eq!(
        second
            .getattr(inode, None)
            .await
            .expect("dirty attributes should recover")
            .attributes()
            .size(),
        9
    );
    assert_eq!(second_probe.content_reads(), 0);
    let recovered_handle = second
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("dirty staging should reopen");
    assert_eq!(second_probe.content_reads(), 0);
    assert_eq!(
        second
            .read_file(inode, recovered_handle, 0, 9)
            .await
            .expect("recovered bytes should read"),
        Bytes::from_static(b"abcdefXYZ")
    );
    assert_eq!(
        second
            .write_file(inode, recovered_handle, 0, Bytes::from_static(b"R"))
            .await
            .expect("recovered generation should advance"),
        1
    );
}

#[tokio::test]
async fn newer_mount_generation_fences_old_writeback_sessions() {
    let store = Arc::new(MemoryWritebackStore::default());
    let (first_readonly, inode) = fixture_readonly(FixtureBackend::new());
    let first = LinuxWritableEngine::activate(
        first_readonly,
        store.clone(),
        SessionGeneration::new(1).expect("generation should be valid"),
    )
    .await
    .expect("first mount should activate");
    let stale_handle = first
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("first handle should open");
    first
        .write_file(inode, stale_handle, 0, Bytes::from_static(b"x"))
        .await
        .expect("first dirty generation should persist");

    let (second_readonly, _) = fixture_readonly(FixtureBackend::new());
    let second = LinuxWritableEngine::activate(
        second_readonly,
        store,
        SessionGeneration::new(2).expect("generation should be valid"),
    )
    .await
    .expect("second mount should activate");
    let current_handle = second
        .open_file(inode, LinuxFileAccess::ReadWrite)
        .await
        .expect("recovered handle should open");
    second
        .write_file(inode, current_handle, 1, Bytes::from_static(b"y"))
        .await
        .expect("new mount should advance dirty state");

    assert!(matches!(
        first
            .write_file(inode, stale_handle, 2, Bytes::from_static(b"z"))
            .await,
        Err(LinuxCloudFilesError::WritebackStore(error))
            if error.kind()
                == aster_forge_cloud_files_core::CloudFilesStoreErrorKind::Conflict
    ));
    assert_eq!(
        second
            .read_file(inode, current_handle, 0, 6)
            .await
            .expect("current staging should remain intact"),
        Bytes::from_static(b"xycdef")
    );
}

#[tokio::test]
async fn recovery_rejects_wrong_scope_unknown_root_and_conflicting_snapshots() {
    let backend = FixtureBackend::new();
    let wrong_scope = CloudScope::new(
        CloudNamespaceId::new("other").expect("namespace should be valid"),
        CloudRootId::new("root").expect("root should be valid"),
    );
    let cases = [
        vec![local_snapshot(
            CloudItemKey::new(
                wrong_scope,
                CloudItemId::new("file").expect("item should be valid"),
            ),
            1,
            6,
        )],
        vec![local_snapshot(
            CloudItemKey::new(
                backend.scope.clone(),
                CloudItemId::new("missing").expect("item should be valid"),
            ),
            1,
            6,
        )],
        vec![local_snapshot(backend.root.key().clone(), 1, 0)],
        vec![
            local_snapshot(backend.file.key().clone(), 1, 6),
            local_snapshot(backend.file.key().clone(), 2, 7),
        ],
    ];

    for snapshots in cases {
        let store = Arc::new(MemoryWritebackStore::default());
        store.configure(|state| state.recovery_override = Some(snapshots));
        let (readonly, _) = fixture_readonly(backend.clone());
        assert!(matches!(
            LinuxWritableEngine::activate(
                readonly,
                store,
                SessionGeneration::new(1).expect("generation should be valid"),
            )
            .await,
            Err(LinuxCloudFilesError::ScopeMismatch)
                | Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
        ));
    }
}

#[tokio::test]
async fn recovered_open_rejects_a_session_from_another_mount_generation() {
    let store = Arc::new(MemoryWritebackStore::default());
    let (first_readonly, inode) = fixture_readonly(FixtureBackend::new());
    let first = LinuxWritableEngine::activate(
        first_readonly,
        store.clone(),
        SessionGeneration::new(1).expect("generation should be valid"),
    )
    .await
    .expect("first mount should activate");
    let handle = first
        .open_file(inode, LinuxFileAccess::Write)
        .await
        .expect("first handle should open");
    first
        .write_file(inode, handle, 0, Bytes::from_static(b"x"))
        .await
        .expect("dirty generation should persist");
    first
        .release_file(handle)
        .await
        .expect("first handle should release");
    store.configure(|state| state.wrong_recovered_session_generation = true);

    let (second_readonly, _) = fixture_readonly(FixtureBackend::new());
    let second = LinuxWritableEngine::activate(
        second_readonly,
        store,
        SessionGeneration::new(2).expect("generation should be valid"),
    )
    .await
    .expect("second mount should activate");
    assert!(matches!(
        second.open_file(inode, LinuxFileAccess::Read).await,
        Err(LinuxCloudFilesError::InvalidBackendResponse { .. })
    ));
}
