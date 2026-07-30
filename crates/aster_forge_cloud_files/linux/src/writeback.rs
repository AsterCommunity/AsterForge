//! Durable local-write staging for the Linux FUSE adapter.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use aster_forge_cloud_files_core::{
    CloudFilesBackend, CloudItem, CloudItemId, CloudItemKey, CloudItemKind, CloudScope,
    ContentRevision, DesiredMutation, LocalContentGeneration, LocalContentSnapshot, MutationOrigin,
    SessionGeneration, StoreResult,
};
use async_trait::async_trait;
use bytes::Bytes;

use crate::{
    LinuxCloudFilesError, LinuxCreateDirectoryRequest, LinuxCreateFileAcceptance,
    LinuxCreateFileRequest, LinuxCreatedFile, LinuxDirectoryEntry, LinuxDirectoryHandle,
    LinuxDirectorySnapshot, LinuxFileHandle, LinuxInode, LinuxInvalidation, LinuxNamespaceItem,
    LinuxNamespaceMutationStore, LinuxNamespaceOverlay, LinuxNamespaceTombstone, LinuxNode,
    LinuxReadOnlyEngine, LinuxRemoteChange, LinuxRemoteDelete, LinuxRemoteEntry,
    LinuxRemoteLocation, LinuxRemoveRequest, LinuxRenameAcceptance, LinuxRenameDestination,
    LinuxRenameRequest, Result,
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::WritableState;
    use crate::LinuxCloudFilesError;

    #[test]
    fn writable_handle_allocator_reports_u64_exhaustion() {
        let mut state = WritableState {
            next: u64::MAX,
            ..WritableState::default()
        };
        assert!(matches!(
            state.allocate(),
            Err(LinuxCloudFilesError::HandleExhausted)
        ));
        assert!(matches!(
            state.allocate_directory(),
            Err(LinuxCloudFilesError::HandleExhausted)
        ));
    }
}

/// Access requested for one opened Linux file handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxFileAccess {
    /// The handle reads the revision-bound remote snapshot only.
    Read,
    /// The handle writes durable local generations and does not permit reads.
    Write,
    /// The handle reads and writes the durable local staging session.
    ReadWrite,
}

impl LinuxFileAccess {
    const fn readable(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    const fn writable(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

/// Opaque product-owned identity of one durable local write session.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LinuxWriteSessionId(String);

impl LinuxWriteSessionId {
    /// Creates a non-empty session identity and preserves it exactly.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(LinuxCloudFilesError::InvalidConfiguration {
                reason: "write session id must not be empty",
            });
        }
        Ok(Self(value))
    }

    /// Returns the opaque session identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LinuxWriteSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LinuxWriteSessionId")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// Revision-bound request to create or reopen durable local staging for an existing file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxWriteOpenRequest {
    key: CloudItemKey,
    base_revision: ContentRevision,
    base_size: u64,
    session_generation: SessionGeneration,
}

impl LinuxWriteOpenRequest {
    /// Creates an existing-file write request from its exact remote snapshot.
    #[must_use]
    pub const fn new(
        key: CloudItemKey,
        base_revision: ContentRevision,
        base_size: u64,
        session_generation: SessionGeneration,
    ) -> Self {
        Self {
            key,
            base_revision,
            base_size,
            session_generation,
        }
    }

    /// Returns the stable item identity.
    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the exact remote content revision hydrated into staging.
    #[must_use]
    pub const fn base_revision(&self) -> &ContentRevision {
        &self.base_revision
    }

    /// Returns the exact hydrated byte length.
    #[must_use]
    pub const fn base_size(&self) -> u64 {
        self.base_size
    }

    /// Returns the active mount session generation that owns this open.
    #[must_use]
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

/// Product-owned durable staging session returned after the base bytes are installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxWriteSession {
    id: LinuxWriteSessionId,
    key: CloudItemKey,
    size: u64,
    session_generation: SessionGeneration,
    snapshot: Option<LocalContentSnapshot>,
}

impl LinuxWriteSession {
    /// Creates a staging session response.
    #[must_use]
    pub const fn new(
        id: LinuxWriteSessionId,
        key: CloudItemKey,
        size: u64,
        session_generation: SessionGeneration,
    ) -> Self {
        Self {
            id,
            key,
            size,
            session_generation,
            snapshot: None,
        }
    }

    /// Creates a session reopened from one immutable durable dirty snapshot.
    #[must_use]
    pub fn from_recovered(
        id: LinuxWriteSessionId,
        snapshot: LocalContentSnapshot,
        session_generation: SessionGeneration,
    ) -> Self {
        Self {
            id,
            key: snapshot.item_key().clone(),
            size: snapshot.size(),
            session_generation,
            snapshot: Some(snapshot),
        }
    }

    /// Returns the opaque product-owned session identity.
    #[must_use]
    pub const fn id(&self) -> &LinuxWriteSessionId {
        &self.id
    }

    /// Returns the item whose bytes are staged.
    #[must_use]
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the current logical staging size.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the mount session generation bound to this writeback session.
    #[must_use]
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }

    /// Returns the immutable dirty snapshot used to reopen this session, when present.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&LocalContentSnapshot> {
        self.snapshot.as_ref()
    }
}

/// Result of one durably accepted write, truncate, flush, or fsync operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxWriteCommit {
    snapshot: LocalContentSnapshot,
}

impl LinuxWriteCommit {
    /// Creates a commit backed by one immutable local generation.
    #[must_use]
    pub const fn new(snapshot: LocalContentSnapshot) -> Self {
        Self { snapshot }
    }

    /// Returns the immutable dirty snapshot that is safe for upload recovery.
    #[must_use]
    pub const fn snapshot(&self) -> &LocalContentSnapshot {
        &self.snapshot
    }

    /// Consumes the commit into its immutable snapshot.
    #[must_use]
    pub fn into_snapshot(self) -> LocalContentSnapshot {
        self.snapshot
    }
}

/// Product-owned durable local-content implementation used by writable FUSE handles.
///
/// A successful mutating method must make both the bytes and returned immutable generation
/// recoverable before it returns. When remote upload is enabled, the product store should also
/// atomically persist a core `ContentUploadIntent` for that exact snapshot before acknowledging the
/// native write. The implementation may compose a filesystem cache, `ContentStorageStore`,
/// `ContentUploadStore`, and `MutationJournalStore`; the Linux adapter does not allocate operation
/// identities, select upload transport, or choose those product persistence details.
#[async_trait]
pub trait LinuxWritebackStore: Send + Sync {
    /// Activates one mount generation and returns durable dirty snapshots for this scope.
    ///
    /// Before this method returns, the implementation must fence lower mount generations from
    /// mutating active writeback state. Products commonly compose this boundary with core's
    /// `MutationJournalStore::activate_session` and their content-storage transaction.
    async fn activate_mount(
        &self,
        scope: &CloudScope,
        session_generation: SessionGeneration,
    ) -> StoreResult<Vec<LocalContentSnapshot>>;

    /// Reopens durable staged bytes for one dirty item without fetching its remote base content.
    ///
    /// `None` means the item has no active dirty snapshot for the requested mount generation.
    /// A returned session must be bound to `session_generation` and carry that snapshot through
    /// [`LinuxWriteSession::snapshot`].
    async fn open_recovered(
        &self,
        key: &CloudItemKey,
        session_generation: SessionGeneration,
    ) -> StoreResult<Option<LinuxWriteSession>>;

    /// Installs or reopens staging from an exact complete remote revision.
    async fn open_existing(
        &self,
        request: &LinuxWriteOpenRequest,
        base_content: Bytes,
    ) -> StoreResult<LinuxWriteSession>;

    /// Reads current staged bytes, including all earlier committed writes on this session.
    async fn read(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        size: u32,
    ) -> StoreResult<Bytes>;

    /// Durably applies one positioned write and returns its immutable dirty generation.
    async fn write(
        &self,
        session: &LinuxWriteSessionId,
        offset: u64,
        bytes: Bytes,
    ) -> StoreResult<LinuxWriteCommit>;

    /// Durably changes the logical file size and returns its immutable dirty generation.
    async fn truncate(
        &self,
        session: &LinuxWriteSessionId,
        size: u64,
    ) -> StoreResult<LinuxWriteCommit>;

    /// Confirms that the latest dirty generation satisfies a flush or fsync durability boundary.
    async fn sync(
        &self,
        session: &LinuxWriteSessionId,
        data_only: bool,
    ) -> StoreResult<Option<LinuxWriteCommit>>;

    /// Releases product-owned mutable session state after all FUSE references are gone.
    async fn close(&self, session: &LinuxWriteSessionId) -> StoreResult<()>;
}

#[derive(Debug, Clone)]
enum OpenFile {
    Writeback {
        inode: LinuxInode,
        access: LinuxFileAccess,
        session: LinuxWriteSessionId,
        size: u64,
        generation: Option<LocalContentGeneration>,
    },
}

type ParentNameKey = (CloudItemId, String);

/// Parent-bucketed name index used by lookup and directory enumeration.
///
/// Keeping the parent as the first hash lookup makes opening one directory proportional to that
/// directory's overlay children rather than to the complete mounted namespace.
#[derive(Debug)]
struct ParentNameIndex<V> {
    by_parent: HashMap<CloudItemId, HashMap<String, V>>,
}

impl<V> Default for ParentNameIndex<V> {
    fn default() -> Self {
        Self {
            by_parent: HashMap::new(),
        }
    }
}

impl<V> ParentNameIndex<V> {
    fn get(&self, key: &ParentNameKey) -> Option<&V> {
        self.by_parent
            .get(&key.0)
            .and_then(|children| children.get(key.1.as_str()))
    }

    fn contains_key(&self, key: &ParentNameKey) -> bool {
        self.get(key).is_some()
    }

    fn insert(&mut self, key: ParentNameKey, value: V) -> Option<V> {
        self.by_parent
            .entry(key.0)
            .or_default()
            .insert(key.1, value)
    }

    fn remove(&mut self, key: &ParentNameKey) -> Option<V> {
        let (removed, empty) = {
            let children = self.by_parent.get_mut(&key.0)?;
            let removed = children.remove(key.1.as_str());
            (removed, children.is_empty())
        };
        if empty {
            self.by_parent.remove(&key.0);
        }
        removed
    }

    fn children(&self, parent: &CloudItemId) -> impl Iterator<Item = (&str, &V)> {
        self.by_parent
            .get(parent)
            .into_iter()
            .flat_map(|children| children.iter().map(|(name, value)| (name.as_str(), value)))
    }
}

#[derive(Default)]
struct WritableState {
    next: u64,
    files: HashMap<LinuxFileHandle, OpenFile>,
    directories: HashMap<LinuxDirectoryHandle, LinuxDirectorySnapshot>,
    dirty_snapshots: HashMap<LinuxInode, LocalContentSnapshot>,
    created_by_inode: HashMap<LinuxInode, LinuxCreatedFile>,
    created_by_key: HashMap<CloudItemKey, LinuxInode>,
    created_by_name: ParentNameIndex<LinuxInode>,
    namespace_by_inode: HashMap<LinuxInode, LinuxNamespaceItem>,
    namespace_by_key: HashMap<CloudItemKey, LinuxInode>,
    namespace_by_name: ParentNameIndex<LinuxInode>,
    remote_by_inode: HashMap<LinuxInode, LinuxRemoteEntry>,
    remote_by_key: HashMap<CloudItemKey, LinuxInode>,
    remote_by_name: ParentNameIndex<LinuxInode>,
    remote_tombstones_by_name: ParentNameIndex<()>,
    remote_deleted_keys: HashSet<CloudItemKey>,
    remote_deleted_inodes: HashSet<LinuxInode>,
    tombstones_by_name: ParentNameIndex<LinuxNamespaceTombstone>,
    deleted_keys: HashSet<CloudItemKey>,
}

impl WritableState {
    fn allocate(&mut self) -> Result<LinuxFileHandle> {
        let current = if self.next == 0 { 1 } else { self.next };
        let Some(next) = current.checked_add(1) else {
            return Err(LinuxCloudFilesError::HandleExhausted);
        };
        self.next = next;
        LinuxFileHandle::new(current)
    }

    fn allocate_directory(&mut self) -> Result<LinuxDirectoryHandle> {
        let current = if self.next == 0 { 1 } else { self.next };
        let Some(next) = current.checked_add(1) else {
            return Err(LinuxCloudFilesError::HandleExhausted);
        };
        self.next = next;
        LinuxDirectoryHandle::new(current)
    }
}

struct WritableEngineInner<B, S> {
    readonly: LinuxReadOnlyEngine<B>,
    store: Arc<S>,
    namespace_store: Option<Arc<dyn LinuxNamespaceMutationStore>>,
    session_generation: SessionGeneration,
    state: Mutex<WritableState>,
}

/// Writable existing-file engine with injected durable local staging.
pub struct LinuxWritableEngine<B, S> {
    inner: Arc<WritableEngineInner<B, S>>,
}

impl<B, S> Clone for LinuxWritableEngine<B, S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<B, S> LinuxWritableEngine<B, S>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
{
    /// Activates one mount generation and restores durable dirty snapshots before exposure.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn activate(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        Self::activate_inner(readonly, store, None, Vec::new(), session_generation).await
    }

    /// Activates writeback plus a durable namespace port for native regular-file creation.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn activate_with_namespace<N>(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        namespace_store: Arc<N>,
        session_generation: SessionGeneration,
    ) -> Result<Self>
    where
        N: LinuxNamespaceMutationStore + 'static,
    {
        Self::activate_inner(
            readonly,
            store,
            Some(namespace_store),
            Vec::new(),
            session_generation,
        )
        .await
    }

    /// Activates namespace/writeback state with product-persisted remote inode mappings.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn activate_with_namespace_and_remote<N>(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        namespace_store: Arc<N>,
        remote_entries: Vec<LinuxRemoteEntry>,
        session_generation: SessionGeneration,
    ) -> Result<Self>
    where
        N: LinuxNamespaceMutationStore + 'static,
    {
        Self::activate_inner(
            readonly,
            store,
            Some(namespace_store),
            remote_entries,
            session_generation,
        )
        .await
    }

    async fn activate_inner(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        namespace_store: Option<Arc<dyn LinuxNamespaceMutationStore>>,
        remote_entries: Vec<LinuxRemoteEntry>,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        let created = match namespace_store.as_ref() {
            Some(namespace_store) => {
                namespace_store
                    .activate_namespace(readonly.inode_table().scope(), session_generation)
                    .await?
            }
            None => Vec::new(),
        };
        let snapshots = store
            .activate_mount(readonly.inode_table().scope(), session_generation)
            .await?;
        let overlay = match namespace_store.as_ref() {
            Some(namespace_store) => {
                namespace_store
                    .activate_namespace_overlay(readonly.inode_table().scope(), session_generation)
                    .await?
            }
            None => LinuxNamespaceOverlay::default(),
        };
        let engine = Self {
            inner: Arc::new(WritableEngineInner {
                readonly,
                store,
                namespace_store,
                session_generation,
                state: Mutex::new(WritableState::default()),
            }),
        };
        engine.restore_namespace_overlay(overlay)?;
        engine.restore_remote_overlay(remote_entries).await?;
        engine.restore_created_files(created).await?;
        engine.restore_dirty_snapshots(snapshots)?;
        Ok(engine)
    }

    /// Returns the underlying read-only engine used for metadata and directory operations.
    #[must_use]
    pub fn readonly(&self) -> &LinuxReadOnlyEngine<B> {
        &self.inner.readonly
    }

    /// Returns the durable mount generation that owns this engine's writeback sessions.
    #[must_use]
    pub fn session_generation(&self) -> SessionGeneration {
        self.inner.session_generation
    }

    /// Restores product-persisted remote entries before the mount is exposed to the kernel.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn restore_remote_overlay(
        &self,
        entries: impl IntoIterator<Item = LinuxRemoteEntry>,
    ) -> Result<()> {
        for entry in entries {
            self.apply_remote_change(LinuxRemoteChange::Upsert(crate::LinuxRemoteUpsert::new(
                entry, None, None,
            )))
            .await?;
        }
        Ok(())
    }

    /// Applies one already-durable remote namespace transition and returns kernel invalidations.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn apply_remote_change(
        &self,
        change: LinuxRemoteChange,
    ) -> Result<Vec<LinuxInvalidation>> {
        match change {
            LinuxRemoteChange::Upsert(upsert) => {
                let (entry, previous, replaced) = upsert.into_parts();
                self.apply_remote_upsert(entry, previous, replaced).await
            }
            LinuxRemoteChange::Delete(deleted) => self.apply_remote_delete(deleted).await,
        }
    }

    /// Loads attributes, overlaying the current staged size when a writable handle is supplied.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn getattr(
        &self,
        inode: LinuxInode,
        handle: Option<LinuxFileHandle>,
    ) -> Result<LinuxNode> {
        let (namespace, created, remote, deleted) = {
            let state = lock(&self.inner.state);
            let restored_key = self
                .inner
                .readonly
                .inode_table()
                .by_inode(inode)
                .map(crate::LinuxInodeRecord::key);
            (
                state.namespace_by_inode.get(&inode).cloned(),
                state.created_by_inode.get(&inode).cloned(),
                state.remote_by_inode.get(&inode).cloned(),
                state.remote_deleted_inodes.contains(&inode)
                    || restored_key.is_some_and(|key| {
                        state.deleted_keys.contains(key) || state.remote_deleted_keys.contains(key)
                    })
                    || state.created_by_inode.get(&inode).is_some_and(|created| {
                        state.deleted_keys.contains(created.item().key())
                            || state.remote_deleted_keys.contains(created.item().key())
                    }),
            )
        };
        if deleted && namespace.is_none() && remote.is_none() {
            return Err(LinuxCloudFilesError::UnknownInode { inode: inode.get() });
        }
        let node = match (namespace, created, remote) {
            (Some(item), _, _) => self
                .inner
                .readonly
                .node_from_item_and_record(item.item(), item.inode_record())?,
            (None, Some(created), _) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            (None, None, Some(remote)) => self
                .inner
                .readonly
                .node_from_item_and_record(remote.item(), remote.inode_record())?,
            (None, None, None) => self.inner.readonly.getattr(inode).await?,
        };
        let size = match handle {
            Some(handle) => {
                let state = lock(&self.inner.state);
                match state.files.get(&handle) {
                    Some(OpenFile::Writeback {
                        inode: opened_inode,
                        size,
                        ..
                    }) if *opened_inode == inode => Some(*size),
                    _ => return Err(LinuxCloudFilesError::StaleHandle),
                }
            }
            None => lock(&self.inner.state)
                .dirty_snapshots
                .get(&inode)
                .map(LocalContentSnapshot::size),
        };
        Ok(match size {
            Some(size) => node.with_size(size),
            None => node,
        })
    }

    /// Resolves one child and overlays its current dirty staging size.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn lookup(&self, parent: LinuxInode, name: &str) -> Result<LinuxNode> {
        crate::validate_linux_name(name)?;
        let parent_node = self.getattr(parent, None).await?;
        if parent_node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let parent_key = parent_node.key();
        let (namespace, created, remote, tombstoned) = {
            let state = lock(&self.inner.state);
            let name_key = (parent_key.item_id().clone(), name.to_owned());
            (
                state
                    .namespace_by_name
                    .get(&name_key)
                    .and_then(|inode| state.namespace_by_inode.get(inode))
                    .cloned(),
                state
                    .created_by_name
                    .get(&name_key)
                    .and_then(|inode| state.created_by_inode.get(inode))
                    .cloned(),
                state
                    .remote_by_name
                    .get(&name_key)
                    .and_then(|inode| state.remote_by_inode.get(inode))
                    .cloned(),
                state.tombstones_by_name.contains_key(&name_key)
                    || state.remote_tombstones_by_name.contains_key(&name_key),
            )
        };
        let node = match (namespace, created, remote, tombstoned) {
            (Some(item), _, _, _) => self
                .inner
                .readonly
                .node_from_item_and_record(item.item(), item.inode_record())?,
            (None, Some(created), _, _) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            (None, None, Some(remote), _) => self
                .inner
                .readonly
                .node_from_item_and_record(remote.item(), remote.inode_record())?,
            (None, None, None, true) => {
                return Err(LinuxCloudFilesError::UnknownInode {
                    inode: parent.get(),
                });
            }
            (None, None, None, false) => self.inner.readonly.lookup(parent, name).await?,
        };
        let size = lock(&self.inner.state)
            .dirty_snapshots
            .get(&node.attributes().inode())
            .map(LocalContentSnapshot::size);
        Ok(match size {
            Some(size) => node.with_size(size),
            None => node,
        })
    }

    /// Opens an existing file for remote reads or durable local writeback.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn open_file(
        &self,
        inode: LinuxInode,
        access: LinuxFileAccess,
    ) -> Result<LinuxFileHandle> {
        let (namespace, created, remote) = {
            let state = lock(&self.inner.state);
            (
                state.namespace_by_inode.get(&inode).cloned(),
                state.created_by_inode.get(&inode).cloned(),
                state.remote_by_inode.get(&inode).cloned(),
            )
        };
        let node = match (namespace.as_ref(), created.as_ref(), remote.as_ref()) {
            (Some(item), _, _) => self
                .inner
                .readonly
                .node_from_item_and_record(item.item(), item.inode_record())?,
            (None, Some(created), _) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            (None, None, Some(remote)) => self
                .inner
                .readonly
                .node_from_item_and_record(remote.item(), remote.inode_record())?,
            (None, None, None) => self.inner.readonly.getattr(inode).await?,
        };
        if node.attributes().kind() != crate::LinuxNodeKind::File {
            return Err(LinuxCloudFilesError::NotFile);
        }
        let key = node.key().clone();
        let (session, recovered) = match self
            .inner
            .store
            .open_recovered(&key, self.inner.session_generation)
            .await?
        {
            Some(session) => (session, true),
            None if created.is_some() => {
                let namespace_store = self
                    .inner
                    .namespace_store
                    .as_ref()
                    .ok_or(LinuxCloudFilesError::NamespaceMutationNotConfigured)?;
                (
                    namespace_store
                        .open_created_file(&key, self.inner.session_generation)
                        .await?,
                    false,
                )
            }
            None => {
                let (request, content) = match remote.as_ref() {
                    Some(remote) => {
                        self.inner
                            .readonly
                            .hydrate_item_for_write(remote.item(), self.inner.session_generation)
                            .await?
                    }
                    None => {
                        self.inner
                            .readonly
                            .hydrate_for_write(inode, self.inner.session_generation)
                            .await?
                    }
                };
                (
                    self.inner.store.open_existing(&request, content).await?,
                    false,
                )
            }
        };
        self.validate_opened_session(inode, &key, &session, recovered)?;
        let opened = OpenFile::Writeback {
            inode,
            access,
            session: session.id().clone(),
            size: session.size(),
            generation: session.snapshot().map(LocalContentSnapshot::generation),
        };
        let mut state = lock(&self.inner.state);
        let handle = state.allocate()?;
        state.files.insert(handle, opened);
        Ok(handle)
    }

    /// Atomically accepts a durable regular-file create and returns its entry plus open handle.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn create_file(
        &self,
        parent: LinuxInode,
        name: &str,
        mode: u32,
        umask: u32,
        access: LinuxFileAccess,
    ) -> Result<(LinuxNode, LinuxFileHandle)> {
        let namespace_store = self
            .inner
            .namespace_store
            .as_ref()
            .ok_or(LinuxCloudFilesError::NamespaceMutationNotConfigured)?;
        let parent_node = self.getattr(parent, None).await?;
        if parent_node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let request = LinuxCreateFileRequest::new(
            parent_node.key().clone(),
            name,
            mode,
            umask,
            access,
            self.inner.session_generation,
        )?;
        let handle = lock(&self.inner.state).allocate()?;
        let acceptance = namespace_store.create_file(&request).await?;
        self.accept_created_file(&request, acceptance, handle)
    }

    /// Atomically accepts a durable directory create and exposes its stable inode.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn create_directory(
        &self,
        parent: LinuxInode,
        name: &str,
        mode: u32,
        umask: u32,
    ) -> Result<LinuxNode> {
        let namespace_store = self
            .inner
            .namespace_store
            .as_ref()
            .ok_or(LinuxCloudFilesError::NamespaceMutationNotConfigured)?;
        let parent_node = self.getattr(parent, None).await?;
        if parent_node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let request = LinuxCreateDirectoryRequest::new(
            parent_node.key().clone(),
            name,
            mode,
            umask,
            self.inner.session_generation,
        )?;
        let item = namespace_store.create_directory(&request).await?;
        self.accept_namespace_create(&request, item)
    }

    /// Atomically renames or moves one namespace entry while preserving stable identity.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn rename(
        &self,
        parent: LinuxInode,
        name: &str,
        new_parent: LinuxInode,
        new_name: &str,
        no_replace: bool,
    ) -> Result<()> {
        crate::validate_linux_name(name)?;
        crate::validate_linux_name(new_name)?;
        if parent == new_parent && name == new_name {
            return Ok(());
        }
        let namespace_store = self
            .inner
            .namespace_store
            .as_ref()
            .ok_or(LinuxCloudFilesError::NamespaceMutationNotConfigured)?;
        let old_parent = self.getattr(parent, None).await?;
        let new_parent = self.getattr(new_parent, None).await?;
        if old_parent.attributes().kind() != crate::LinuxNodeKind::Directory
            || new_parent.attributes().kind() != crate::LinuxNodeKind::Directory
        {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let source = self.lookup(parent, name).await?;
        let destination = match self.lookup(new_parent.attributes().inode(), new_name).await {
            Ok(node) => Some(LinuxRenameDestination::new(
                node.key().clone(),
                node.generation(),
                node.attributes().kind(),
            )),
            Err(error) if error.error_code() == crate::LinuxErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let request = LinuxRenameRequest::new(
            source.key().clone(),
            source.generation(),
            source.attributes().kind(),
            old_parent.key().clone(),
            name,
            new_parent.key().clone(),
            new_name,
            destination,
            no_replace,
            self.inner.session_generation,
        )?;
        let acceptance = namespace_store.rename(&request).await?;
        self.accept_rename(&request, acceptance)
    }

    /// Atomically accepts unlink or rmdir and hides the durable tombstone from new lookups.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn remove(
        &self,
        parent: LinuxInode,
        name: &str,
        expected_kind: crate::LinuxNodeKind,
    ) -> Result<()> {
        let namespace_store = self
            .inner
            .namespace_store
            .as_ref()
            .ok_or(LinuxCloudFilesError::NamespaceMutationNotConfigured)?;
        let parent_node = self.getattr(parent, None).await?;
        if parent_node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let child = self.lookup(parent, name).await?;
        if child.attributes().kind() != expected_kind {
            return Err(match expected_kind {
                crate::LinuxNodeKind::File => LinuxCloudFilesError::NotFile,
                crate::LinuxNodeKind::Directory => LinuxCloudFilesError::NotDirectory,
            });
        }
        let request = LinuxRemoveRequest::new(
            child.key().clone(),
            child.generation(),
            parent_node.key().clone(),
            name,
            expected_kind,
            self.inner.session_generation,
        )?;
        let tombstone = namespace_store.remove(&request).await?;
        self.accept_remove(&request, tombstone)
    }

    /// Opens a directory snapshot that includes durable local creates accepted before this call.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    #[expect(
        clippy::too_many_lines,
        reason = "the directory snapshot keeps backend, created, renamed, and tombstoned entries in one ordering pass"
    )]
    pub async fn open_directory(&self, inode: LinuxInode) -> Result<LinuxDirectoryHandle> {
        let node = self.getattr(inode, None).await?;
        if node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let directory_item = match self.overlay_item(inode) {
            Some((item, _)) => item,
            None => self.inner.readonly.load_item_for_overlay(inode).await?,
        };
        let backend_children = self
            .inner
            .readonly
            .load_children_for_overlay(directory_item.key())
            .await?;
        let parent = match directory_item.parent_id() {
            Some(parent_id) => self
                .inode_for_key(&CloudItemKey::new(
                    directory_item.key().scope().clone(),
                    parent_id.clone(),
                ))
                .ok_or(LinuxCloudFilesError::MissingInodeRecord)?,
            None => inode,
        };
        let (tombstoned_names, overlay_names, mut local) = {
            let state = lock(&self.inner.state);
            let mut overlay_names = HashSet::new();
            let mut local = Vec::new();

            for (name, child_inode) in state.namespace_by_name.children(node.key().item_id()) {
                if let Some(item) = state.namespace_by_inode.get(child_inode) {
                    overlay_names.insert(name.to_owned());
                    local.push((
                        name.to_owned(),
                        item.item().clone(),
                        item.inode_record().clone(),
                    ));
                }
            }
            for (name, child_inode) in state.created_by_name.children(node.key().item_id()) {
                if !overlay_names.contains(name)
                    && let Some(created) = state.created_by_inode.get(child_inode)
                {
                    overlay_names.insert(name.to_owned());
                    local.push((
                        name.to_owned(),
                        created.item().clone(),
                        created.inode_record().clone(),
                    ));
                }
            }
            for (name, child_inode) in state.remote_by_name.children(node.key().item_id()) {
                if !overlay_names.contains(name)
                    && let Some(remote) = state.remote_by_inode.get(child_inode)
                {
                    overlay_names.insert(name.to_owned());
                    local.push((
                        name.to_owned(),
                        remote.item().clone(),
                        remote.inode_record().clone(),
                    ));
                }
            }

            let mut tombstoned_names = state
                .tombstones_by_name
                .children(node.key().item_id())
                .map(|(name, _)| name.to_owned())
                .chain(
                    state
                        .remote_tombstones_by_name
                        .children(node.key().item_id())
                        .map(|(name, ())| name.to_owned()),
                )
                .collect::<HashSet<_>>();
            // A current overlay entry is authoritative over an older tombstone for the same
            // parent/name. Transition code removes those tombstones, while this subtraction keeps
            // a restored legacy state internally consistent during migration.
            tombstoned_names.retain(|name| !overlay_names.contains(name));
            (tombstoned_names, overlay_names, local)
        };
        let mut entries = backend_children
            .into_iter()
            .filter(|item| {
                !tombstoned_names.contains(item.name()) && !overlay_names.contains(item.name())
            })
            .map(|item| {
                let record = self
                    .record_for_key(item.key())
                    .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
                let child = self
                    .inner
                    .readonly
                    .node_from_item_and_record(&item, &record)?;
                Ok(LinuxDirectoryEntry::from_node(
                    item.name().to_owned(),
                    &child,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        local.retain(|(name, _, _)| !tombstoned_names.contains(name));
        local.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, item, record) in local {
            if entries.iter().any(|entry| entry.name() == name) {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "durable overlay collided with another visible directory entry",
                });
            }
            let child = self
                .inner
                .readonly
                .node_from_item_and_record(&item, &record)?;
            entries.push(LinuxDirectoryEntry::from_node(name, &child));
        }
        let snapshot = LinuxDirectorySnapshot::with_entries(inode, parent, entries);
        let mut state = lock(&self.inner.state);
        let handle = state.allocate_directory()?;
        state.directories.insert(handle, snapshot);
        Ok(handle)
    }

    /// Returns the immutable writable-directory snapshot for one handle.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn directory_snapshot(
        &self,
        inode: LinuxInode,
        handle: LinuxDirectoryHandle,
    ) -> Result<LinuxDirectorySnapshot> {
        let state = lock(&self.inner.state);
        let snapshot = state
            .directories
            .get(&handle)
            .ok_or(LinuxCloudFilesError::StaleHandle)?;
        if snapshot.directory() != inode {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(snapshot.clone())
    }

    fn overlay_item(&self, inode: LinuxInode) -> Option<(CloudItem, crate::LinuxInodeRecord)> {
        let state = lock(&self.inner.state);
        if let Some(item) = state.namespace_by_inode.get(&inode) {
            return Some((item.item().clone(), item.inode_record().clone()));
        }
        if let Some(created) = state.created_by_inode.get(&inode) {
            return Some((created.item().clone(), created.inode_record().clone()));
        }
        state
            .remote_by_inode
            .get(&inode)
            .map(|entry| (entry.item().clone(), entry.inode_record().clone()))
    }

    fn record_for_key(&self, key: &CloudItemKey) -> Option<crate::LinuxInodeRecord> {
        let state = lock(&self.inner.state);
        self.record_for_key_in_state(&state, key)
    }

    fn record_for_key_in_state(
        &self,
        state: &WritableState,
        key: &CloudItemKey,
    ) -> Option<crate::LinuxInodeRecord> {
        state
            .namespace_by_key
            .get(key)
            .and_then(|inode| state.namespace_by_inode.get(inode))
            .map(|item| item.inode_record().clone())
            .or_else(|| {
                state
                    .created_by_key
                    .get(key)
                    .and_then(|inode| state.created_by_inode.get(inode))
                    .map(|item| item.inode_record().clone())
            })
            .or_else(|| {
                state
                    .remote_by_key
                    .get(key)
                    .and_then(|inode| state.remote_by_inode.get(inode))
                    .map(|item| item.inode_record().clone())
            })
            .or_else(|| self.inner.readonly.inode_table().by_key(key).cloned())
    }

    fn inode_for_key(&self, key: &CloudItemKey) -> Option<LinuxInode> {
        self.record_for_key(key).map(|record| record.inode())
    }

    fn inode_for_key_in_state(
        &self,
        state: &WritableState,
        key: &CloudItemKey,
    ) -> Option<LinuxInode> {
        self.record_for_key_in_state(state, key)
            .map(|record| record.inode())
    }

    fn key_for_inode_in_state(
        &self,
        state: &WritableState,
        inode: LinuxInode,
    ) -> Option<CloudItemKey> {
        state
            .namespace_by_inode
            .get(&inode)
            .map(|item| item.item().key().clone())
            .or_else(|| {
                state
                    .created_by_inode
                    .get(&inode)
                    .map(|item| item.item().key().clone())
            })
            .or_else(|| {
                state
                    .remote_by_inode
                    .get(&inode)
                    .map(|item| item.item().key().clone())
            })
            .or_else(|| {
                self.inner
                    .readonly
                    .inode_table()
                    .by_inode(inode)
                    .map(|record| record.key().clone())
            })
    }

    fn validate_remote_entry(&self, state: &WritableState, entry: &LinuxRemoteEntry) -> Result<()> {
        if entry.item().key() != entry.inode_record().key()
            || entry.item().key().scope() != self.inner.readonly.inode_table().scope()
            || entry.item().is_root()
            || entry.inode_record().inode() == crate::LINUX_ROOT_INODE
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote overlay entry violated scope, identity, or root boundaries",
            });
        }
        self.inner
            .readonly
            .node_from_item_and_record(entry.item(), entry.inode_record())?;
        if let Some(existing_key) = self.key_for_inode_in_state(state, entry.inode_record().inode())
            && existing_key != *entry.item().key()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote overlay reused an inode assigned to another stable item",
            });
        }
        if let Some(existing) = self.record_for_key_in_state(state, entry.item().key())
            && existing != *entry.inode_record()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote overlay changed a stable item inode or generation",
            });
        }
        Ok(())
    }

    fn validate_remote_delete(
        &self,
        state: &WritableState,
        deleted: &LinuxRemoteDelete,
    ) -> Result<()> {
        if deleted.key() != deleted.inode_record().key()
            || deleted.key().scope() != self.inner.readonly.inode_table().scope()
            || deleted.parent_key().scope() != self.inner.readonly.inode_table().scope()
            || self.record_for_key_in_state(state, deleted.key()).as_ref()
                != Some(deleted.inode_record())
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote deletion substituted scope, stable identity, or inode generation",
            });
        }
        Ok(())
    }

    async fn lookup_remote_validation_target(
        &self,
        parent: LinuxInode,
        name: &str,
    ) -> Result<Option<LinuxNode>> {
        match self.lookup(parent, name).await {
            Ok(node) => Ok(Some(node)),
            Err(error) if error.error_code() == crate::LinuxErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the remote upsert applies one atomic namespace transition with all identity and replacement checks visible"
    )]
    async fn apply_remote_upsert(
        &self,
        entry: LinuxRemoteEntry,
        previous: Option<LinuxRemoteLocation>,
        replaced: Option<LinuxRemoteDelete>,
    ) -> Result<Vec<LinuxInvalidation>> {
        let key = entry.item().key().clone();
        let inode = entry.inode_record().inode();
        let parent_id = entry
            .item()
            .parent_id()
            .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote overlay item omitted its parent",
            })?
            .clone();
        let parent_key = CloudItemKey::new(key.scope().clone(), parent_id.clone());
        let parent = self
            .inode_for_key(&parent_key)
            .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
        let target = self
            .lookup_remote_validation_target(parent, entry.item().name())
            .await?;
        match (target.as_ref(), replaced.as_ref()) {
            (Some(existing), None) if existing.key() != &key => {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert omitted the item replaced at its destination",
                });
            }
            (Some(existing), Some(replaced))
                if existing.key() == replaced.key()
                    && existing.attributes().inode() == replaced.inode_record().inode()
                    && existing.generation() == replaced.inode_record().generation()
                    && existing.attributes().kind() == replaced.kind()
                    && replaced.parent_key() == &parent_key
                    && replaced.name() == entry.item().name() => {}
            (None | Some(_), None) => {}
            _ => {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert replacement did not match its current destination",
                });
            }
        }

        let previous_node = if let Some(location) = previous.as_ref() {
            if location.parent_key().scope() != key.scope() {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert previous location escaped the active scope",
                });
            }
            let previous_parent = self
                .inode_for_key(location.parent_key())
                .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
            let node = self
                .lookup_remote_validation_target(previous_parent, location.name())
                .await?
                .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert previous location did not exist",
                })?;
            if node.key() != &key || node.attributes().inode() != inode {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert previous location named another stable item",
                });
            }
            Some((previous_parent, node))
        } else {
            None
        };

        // All in-memory validation and every secondary-index update share one critical section.
        // Backend lookups above establish immutable product facts; the lock makes local and remote
        // overlay transitions linearizable with respect to those facts.
        let name_key = (parent_id, entry.item().name().to_owned());
        let mut state = lock(&self.inner.state);
        self.validate_remote_entry(&state, &entry)?;
        if state.namespace_by_key.contains_key(&key)
            || state.created_by_key.contains_key(&key)
            || state.deleted_keys.contains(&key)
            || state.tombstones_by_name.contains_key(&name_key)
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote change collided with a pending local namespace mutation",
            });
        }
        if let Some(replaced) = replaced.as_ref() {
            self.validate_remote_delete(&state, replaced)?;
            if replaced.key() == &key {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote upsert replaced its own stable identity",
                });
            }
        }

        for candidate in [
            state.namespace_by_name.get(&name_key),
            state.created_by_name.get(&name_key),
        ]
        .into_iter()
        .flatten()
        {
            if self.key_for_inode_in_state(&state, *candidate).as_ref() != Some(&key) {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "remote change destination collided with a local namespace mutation",
                });
            }
        }
        if let Some(existing_inode) = state.remote_by_name.get(&name_key)
            && self
                .key_for_inode_in_state(&state, *existing_inode)
                .as_ref()
                != Some(&key)
            && replaced.as_ref().map(LinuxRemoteDelete::key)
                != self
                    .key_for_inode_in_state(&state, *existing_inode)
                    .as_ref()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote upsert did not identify the current destination replacement",
            });
        }

        if let Some(current_inode) = state.remote_by_key.get(&key)
            && let Some(current) = state.remote_by_inode.get(current_inode)
        {
            let current_parent =
                current
                    .item()
                    .parent_id()
                    .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "current remote overlay item omitted its parent",
                    })?;
            let current_location = (current_parent, current.item().name());
            match previous.as_ref() {
                Some(location)
                    if current_location == (location.parent_key().item_id(), location.name()) => {}
                None if current_location == (&name_key.0, name_key.1.as_str()) => {}
                _ => {
                    return Err(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "remote upsert previous location was not the current location",
                    });
                }
            }
        }

        let mut invalidations = Vec::new();
        if let Some((previous_parent, _)) = previous_node {
            let location =
                previous
                    .as_ref()
                    .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "validated previous location disappeared",
                    })?;
            invalidations.push(LinuxInvalidation::Entry {
                parent: previous_parent,
                name: location.name().to_owned(),
            });
        }
        if let Some(replaced) = replaced.as_ref() {
            let replaced_parent = self
                .inode_for_key_in_state(&state, replaced.parent_key())
                .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
            invalidations.push(LinuxInvalidation::Delete {
                parent: replaced_parent,
                child: replaced.inode_record().inode(),
                name: replaced.name().to_owned(),
            });
        }
        invalidations.push(LinuxInvalidation::Entry {
            parent,
            name: entry.item().name().to_owned(),
        });
        invalidations.push(LinuxInvalidation::Inode { inode });

        if let Some(location) = previous {
            let old_name = (
                location.parent_key().item_id().clone(),
                location.name().to_owned(),
            );
            state.remote_by_name.remove(&old_name);
            state.remote_tombstones_by_name.insert(old_name, ());
        }
        if let Some(replaced) = replaced {
            let replaced_name = (
                replaced.parent_key().item_id().clone(),
                replaced.name().to_owned(),
            );
            if let Some(replaced_inode) = state.remote_by_key.remove(replaced.key()) {
                state.remote_by_inode.remove(&replaced_inode);
            }
            state.remote_by_name.remove(&replaced_name);
            state.remote_tombstones_by_name.insert(replaced_name, ());
            state.remote_deleted_keys.insert(replaced.key().clone());
            state
                .remote_deleted_inodes
                .insert(replaced.inode_record().inode());
        }
        state.remote_tombstones_by_name.remove(&name_key);
        state.remote_deleted_keys.remove(&key);
        state.remote_deleted_inodes.remove(&inode);
        state.remote_by_key.insert(key, inode);
        state.remote_by_name.insert(name_key, inode);
        state.remote_by_inode.insert(inode, entry);
        Ok(invalidations)
    }

    async fn apply_remote_delete(
        &self,
        deleted: LinuxRemoteDelete,
    ) -> Result<Vec<LinuxInvalidation>> {
        let parent = self
            .inode_for_key(deleted.parent_key())
            .ok_or(LinuxCloudFilesError::MissingInodeRecord)?;
        let current = self
            .lookup_remote_validation_target(parent, deleted.name())
            .await?
            .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote deletion location did not exist",
            })?;
        if current.key() != deleted.key()
            || current.attributes().inode() != deleted.inode_record().inode()
            || current.generation() != deleted.inode_record().generation()
            || current.attributes().kind() != deleted.kind()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote deletion did not match the current namespace entry",
            });
        }
        let inode = deleted.inode_record().inode();
        let name_key = (
            deleted.parent_key().item_id().clone(),
            deleted.name().to_owned(),
        );
        let mut state = lock(&self.inner.state);
        self.validate_remote_delete(&state, &deleted)?;
        if state.namespace_by_key.contains_key(deleted.key())
            || state.created_by_key.contains_key(deleted.key())
            || state.deleted_keys.contains(deleted.key())
            || state.tombstones_by_name.contains_key(&name_key)
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote deletion collided with a pending local namespace mutation",
            });
        }
        if let Some(current_inode) = state.remote_by_name.get(&name_key)
            && *current_inode != inode
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remote deletion location changed before commit",
            });
        }
        state.remote_by_key.remove(deleted.key());
        state.remote_by_inode.remove(&inode);
        state.remote_by_name.remove(&name_key);
        state.remote_tombstones_by_name.insert(name_key, ());
        state.remote_deleted_keys.insert(deleted.key().clone());
        state.remote_deleted_inodes.insert(inode);
        Ok(vec![LinuxInvalidation::Delete {
            parent,
            child: inode,
            name: deleted.name().to_owned(),
        }])
    }

    /// Releases one writable-directory snapshot handle.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn release_directory(&self, handle: LinuxDirectoryHandle) -> Result<()> {
        if lock(&self.inner.state)
            .directories
            .remove(&handle)
            .is_none()
        {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        Ok(())
    }

    /// Reads from the remote revision or current durable staging, depending on the handle mode.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn read_file(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
        offset: u64,
        size: u32,
    ) -> Result<Bytes> {
        let opened = {
            let state = lock(&self.inner.state);
            state
                .files
                .get(&handle)
                .cloned()
                .ok_or(LinuxCloudFilesError::StaleHandle)?
        };
        match opened {
            OpenFile::Writeback {
                inode: opened_inode,
                access,
                session,
                size: logical_size,
                ..
            } if opened_inode == inode && access.readable() => {
                if size == 0 || offset >= logical_size {
                    return Ok(Bytes::new());
                }
                let expected = u64::from(size).min(logical_size - offset);
                let bytes = self.inner.store.read(&session, offset, size).await?;
                let actual = u64::try_from(bytes.len()).map_err(|_| {
                    LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "writeback read length cannot be represented as u64",
                    }
                })?;
                if actual != expected {
                    return Err(LinuxCloudFilesError::InvalidBackendResponse {
                        reason: "writeback read returned a partial non-EOF range",
                    });
                }
                Ok(bytes)
            }
            OpenFile::Writeback { access, .. } if !access.readable() => {
                Err(LinuxCloudFilesError::AccessModeMismatch)
            }
            OpenFile::Writeback { .. } => Err(LinuxCloudFilesError::StaleHandle),
        }
    }

    /// Durably applies a positioned write before reporting accepted bytes to FUSE.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn write_file(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
        offset: u64,
        bytes: Bytes,
    ) -> Result<u32> {
        let byte_count =
            u32::try_from(bytes.len()).map_err(|_| LinuxCloudFilesError::InvalidConfiguration {
                reason: "FUSE write length cannot be represented as u32",
            })?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(u64::from(byte_count)).ok_or(
            LinuxCloudFilesError::InvalidConfiguration {
                reason: "FUSE write range exceeds u64",
            },
        )?;
        let session = self.write_session(inode, handle)?;
        let commit = self.inner.store.write(&session, offset, bytes).await?;
        self.apply_commit(handle, inode, commit, Some(end), false)?;
        Ok(byte_count)
    }

    /// Durably truncates or extends the staged file.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn truncate_file(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
        size: u64,
    ) -> Result<LinuxNode> {
        let session = self.write_session(inode, handle)?;
        let commit = self.inner.store.truncate(&session, size).await?;
        self.apply_commit(handle, inode, commit, Some(size), false)?;
        self.getattr(inode, Some(handle)).await
    }

    /// Opens, truncates, and releases one existing file for handle-less `setattr` requests.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn truncate_once(&self, inode: LinuxInode, size: u64) -> Result<LinuxNode> {
        let handle = self.open_file(inode, LinuxFileAccess::Write).await?;
        let result = self.truncate_file(inode, handle, size).await;
        let release = self.release_file(handle).await;
        match (result, release) {
            (Ok(node), Ok(())) => Ok(node),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    /// Applies an idempotent flush or explicit fsync durability boundary.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn sync_file(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
        data_only: bool,
    ) -> Result<()> {
        let Some(session) = self.sync_session(inode, handle)? else {
            return Ok(());
        };
        if let Some(commit) = self.inner.store.sync(&session, data_only).await? {
            self.apply_commit(handle, inode, commit, None, true)?;
        }
        Ok(())
    }

    fn sync_session(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
    ) -> Result<Option<LinuxWriteSessionId>> {
        let state = lock(&self.inner.state);
        match state.files.get(&handle) {
            Some(OpenFile::Writeback {
                inode: opened_inode,
                access,
                session,
                ..
            }) if *opened_inode == inode && access.writable() => Ok(Some(session.clone())),
            Some(OpenFile::Writeback {
                inode: opened_inode,
                access: LinuxFileAccess::Read,
                ..
            }) if *opened_inode == inode => Ok(None),
            Some(OpenFile::Writeback { .. }) | None => Err(LinuxCloudFilesError::StaleHandle),
        }
    }

    /// Releases one file handle and its product-owned writeback session.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub async fn release_file(&self, handle: LinuxFileHandle) -> Result<()> {
        let opened = lock(&self.inner.state)
            .files
            .remove(&handle)
            .ok_or(LinuxCloudFilesError::StaleHandle)?;
        match opened {
            OpenFile::Writeback { session, .. } => {
                self.inner.store.close(&session).await?;
                Ok(())
            }
        }
    }

    fn write_session(
        &self,
        inode: LinuxInode,
        handle: LinuxFileHandle,
    ) -> Result<LinuxWriteSessionId> {
        let state = lock(&self.inner.state);
        match state.files.get(&handle) {
            Some(OpenFile::Writeback {
                inode: opened_inode,
                access,
                session,
                ..
            }) if *opened_inode == inode && access.writable() => Ok(session.clone()),
            Some(OpenFile::Writeback { .. }) => Err(LinuxCloudFilesError::AccessModeMismatch),
            None => Err(LinuxCloudFilesError::StaleHandle),
        }
    }

    fn accept_created_file(
        &self,
        request: &LinuxCreateFileRequest,
        acceptance: LinuxCreateFileAcceptance,
        handle: LinuxFileHandle,
    ) -> Result<(LinuxNode, LinuxFileHandle)> {
        let (created, session) = acceptance.into_parts();
        self.validate_created_file(&created, Some(request))?;
        self.validate_created_session(&created, &session)?;
        let node = self
            .inner
            .readonly
            .node_from_item_and_record(created.item(), created.inode_record())?;
        let inode = created.inode_record().inode();
        let key = created.item().key().clone();
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        let mut state = lock(&self.inner.state);
        self.ensure_created_file_is_unique(&state, &created, &name_key)?;
        if state.files.values().any(|opened| match opened {
            OpenFile::Writeback {
                session: opened_session,
                ..
            } => opened_session == session.id(),
        }) {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store reused an active write session identity",
            });
        }
        state.tombstones_by_name.remove(&name_key);
        state.remote_tombstones_by_name.remove(&name_key);
        state.created_by_key.insert(key, inode);
        state.created_by_name.insert(name_key, inode);
        state.created_by_inode.insert(inode, created);
        state.files.insert(
            handle,
            OpenFile::Writeback {
                inode,
                access: request.access(),
                session: session.id().clone(),
                size: 0,
                generation: None,
            },
        );
        Ok((node, handle))
    }

    fn accept_namespace_create(
        &self,
        request: &LinuxCreateDirectoryRequest,
        item: LinuxNamespaceItem,
    ) -> Result<LinuxNode> {
        self.validate_namespace_create(request, &item)?;
        let node = self
            .inner
            .readonly
            .node_from_item_and_record(item.item(), item.inode_record())?;
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        let mut state = lock(&self.inner.state);
        self.ensure_namespace_item_is_unique(&state, &item, &name_key, None)?;
        let inode = item.inode_record().inode();
        state.tombstones_by_name.remove(&name_key);
        state.remote_tombstones_by_name.remove(&name_key);
        state
            .namespace_by_key
            .insert(item.item().key().clone(), inode);
        state.namespace_by_name.insert(name_key, inode);
        state.namespace_by_inode.insert(inode, item);
        Ok(node)
    }

    fn accept_rename(
        &self,
        request: &LinuxRenameRequest,
        acceptance: LinuxRenameAcceptance,
    ) -> Result<()> {
        let (item, source, replaced) = acceptance.into_parts();
        self.validate_rename_acceptance(request, &item, &source, replaced.as_ref())?;
        let new_name_key = (
            request.new_parent_key().item_id().clone(),
            request.new_name().to_owned(),
        );
        let old_name_key = (
            request.old_parent_key().item_id().clone(),
            request.old_name().to_owned(),
        );
        let inode = item.inode_record().inode();
        let mut state = lock(&self.inner.state);
        self.ensure_namespace_item_is_unique(&state, &item, &new_name_key, Some(request.key()))?;
        state.namespace_by_name.remove(&old_name_key);
        state.created_by_name.remove(&old_name_key);
        state.tombstones_by_name.insert(old_name_key, source);
        if let Some(replaced) = replaced {
            let replaced_name_key = (
                replaced.parent_key().item_id().clone(),
                replaced.name().to_owned(),
            );
            if let Some(replaced_inode) = state
                .namespace_by_key
                .remove(replaced.key())
                .or_else(|| state.created_by_key.remove(replaced.key()))
                .or_else(|| {
                    self.inner
                        .readonly
                        .inode_table()
                        .by_key(replaced.key())
                        .map(crate::LinuxInodeRecord::inode)
                })
            {
                state.namespace_by_inode.remove(&replaced_inode);
                state.created_by_name.remove(&replaced_name_key);
            }
            state.deleted_keys.insert(replaced.key().clone());
            state.tombstones_by_name.insert(replaced_name_key, replaced);
        }
        state.deleted_keys.remove(request.key());
        state.tombstones_by_name.remove(&new_name_key);
        state.remote_tombstones_by_name.remove(&new_name_key);
        state.namespace_by_key.insert(request.key().clone(), inode);
        state.namespace_by_name.insert(new_name_key.clone(), inode);
        state.namespace_by_inode.insert(inode, item);
        if state.created_by_key.contains_key(request.key()) {
            state.created_by_name.insert(new_name_key, inode);
        }
        Ok(())
    }

    fn accept_remove(
        &self,
        request: &LinuxRemoveRequest,
        tombstone: LinuxNamespaceTombstone,
    ) -> Result<()> {
        Self::validate_remove_acceptance(request, &tombstone)?;
        let name_key = (
            request.parent_key().item_id().clone(),
            request.name().to_owned(),
        );
        let mut state = lock(&self.inner.state);
        if let Some(inode) = state
            .namespace_by_key
            .remove(request.key())
            .or_else(|| state.created_by_key.get(request.key()).copied())
        {
            state.namespace_by_inode.remove(&inode);
        }
        state.namespace_by_name.remove(&name_key);
        state.created_by_name.remove(&name_key);
        state.deleted_keys.insert(request.key().clone());
        state.tombstones_by_name.insert(name_key, tombstone);
        Ok(())
    }

    fn validate_namespace_create(
        &self,
        request: &LinuxCreateDirectoryRequest,
        item: &LinuxNamespaceItem,
    ) -> Result<()> {
        self.validate_namespace_item(item)?;
        let DesiredMutation::Create {
            scope,
            parent_id,
            name,
            kind,
        } = item.mutation_intent().desired()
        else {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "directory create did not persist a create mutation",
            });
        };
        if item.item().kind() != CloudItemKind::Directory
            || item.item().content().is_some()
            || item.item().parent_id() != Some(request.parent_key().item_id())
            || item.item().name() != request.name()
            || scope != request.parent_key().scope()
            || parent_id != request.parent_key().item_id()
            || name != request.name()
            || *kind != CloudItemKind::Directory
            || item.mutation_intent().session_generation() != request.session_generation()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "directory create acceptance did not match the native request",
            });
        }
        Ok(())
    }

    fn validate_rename_acceptance(
        &self,
        request: &LinuxRenameRequest,
        item: &LinuxNamespaceItem,
        source: &LinuxNamespaceTombstone,
        replaced: Option<&LinuxNamespaceTombstone>,
    ) -> Result<()> {
        self.validate_namespace_item(item)?;
        let DesiredMutation::ModifyMetadata {
            key,
            parent_id,
            name,
        } = item.mutation_intent().desired()
        else {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "rename did not persist a metadata mutation",
            });
        };
        let expected_kind = match request.kind() {
            crate::LinuxNodeKind::File => CloudItemKind::File,
            crate::LinuxNodeKind::Directory => CloudItemKind::Directory,
        };
        if item.item().key() != request.key()
            || item.inode_record().key() != request.key()
            || item.inode_record().generation() != request.inode_generation()
            || item.item().kind() != expected_kind
            || item.item().parent_id() != Some(request.new_parent_key().item_id())
            || item.item().name() != request.new_name()
            || key != request.key()
            || parent_id.as_ref() != Some(request.new_parent_key().item_id())
            || name.as_deref() != Some(request.new_name())
            || item.mutation_intent().origin() != MutationOrigin::PlatformCommand
            || item.mutation_intent().session_generation() != request.session_generation()
            || source.key() != request.key()
            || source.inode_generation() != request.inode_generation()
            || source.parent_key() != request.old_parent_key()
            || source.name() != request.old_name()
            || source.kind() != request.kind()
            || source.mutation_intent().operation_id() != item.mutation_intent().operation_id()
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "rename acceptance substituted namespace identity or intent",
            });
        }
        match (request.destination(), replaced) {
            (None, None) => {}
            (Some(_), _) if request.no_replace() => {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "no-replace rename accepted an existing destination",
                });
            }
            (Some(destination), Some(replaced))
                if replaced.key() == destination.key()
                    && replaced.inode_generation() == destination.inode_generation()
                    && replaced.kind() == destination.kind()
                    && replaced.parent_key() == request.new_parent_key()
                    && replaced.name() == request.new_name() => {}
            _ => {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "rename replacement tombstone did not match the destination",
                });
            }
        }
        Ok(())
    }

    fn validate_remove_acceptance(
        request: &LinuxRemoveRequest,
        tombstone: &LinuxNamespaceTombstone,
    ) -> Result<()> {
        let intent = tombstone.mutation_intent();
        if tombstone.key() != request.key()
            || tombstone.inode_generation() != request.inode_generation()
            || tombstone.parent_key() != request.parent_key()
            || tombstone.name() != request.name()
            || tombstone.kind() != request.kind()
            || intent.origin() != MutationOrigin::PlatformCommand
            || intent.session_generation() != request.session_generation()
            || !matches!(intent.desired(), DesiredMutation::Delete { key } if key == request.key())
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "remove acceptance substituted namespace identity or intent",
            });
        }
        Ok(())
    }

    fn validate_namespace_item(&self, item: &LinuxNamespaceItem) -> Result<()> {
        item.mutation_intent()
            .validate_for_persistence()
            .map_err(|_| LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace item returned an invalid mutation intent",
            })?;
        if item.item().is_root()
            || item.item().key() != item.inode_record().key()
            || item.inode_record().inode() == crate::LINUX_ROOT_INODE
            || item.item().key().scope() != self.inner.readonly.inode_table().scope()
            || item.mutation_intent().session_generation() > self.inner.session_generation
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace item returned an invalid stable mapping",
            });
        }
        crate::validate_linux_name(item.item().name())?;
        Ok(())
    }

    fn ensure_namespace_item_is_unique(
        &self,
        state: &WritableState,
        item: &LinuxNamespaceItem,
        name_key: &(CloudItemId, String),
        replacing_key: Option<&CloudItemKey>,
    ) -> Result<()> {
        let record = item.inode_record();
        let static_key_conflict = self
            .inner
            .readonly
            .inode_table()
            .by_key(item.item().key())
            .is_some_and(|existing| replacing_key != Some(existing.key()));
        let static_inode_conflict = self
            .inner
            .readonly
            .inode_table()
            .by_inode(record.inode())
            .is_some_and(|existing| replacing_key != Some(existing.key()));
        let dynamic_key_conflict = state
            .namespace_by_key
            .get(item.item().key())
            .is_some_and(|_| replacing_key != Some(item.item().key()));
        let dynamic_inode_conflict = state
            .namespace_by_inode
            .get(&record.inode())
            .is_some_and(|existing| replacing_key != Some(existing.item().key()));
        let name_conflict = state
            .namespace_by_name
            .get(name_key)
            .or_else(|| state.created_by_name.get(name_key))
            .is_some_and(|inode| *inode != record.inode());
        if static_key_conflict
            || static_inode_conflict
            || dynamic_key_conflict
            || dynamic_inode_conflict
            || name_conflict
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store returned a duplicate key, inode, or parent/name mapping",
            });
        }
        Ok(())
    }

    fn validate_created_session(
        &self,
        created: &LinuxCreatedFile,
        session: &LinuxWriteSession,
    ) -> Result<()> {
        if session.key() != created.item().key() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "create staging session returned a different stable item identity",
            });
        }
        if session.size() != 0 || session.snapshot().is_some() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "create staging session was not a clean empty file",
            });
        }
        if session.session_generation() != self.inner.session_generation {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "create staging session used another mount generation",
            });
        }
        Ok(())
    }

    fn validate_created_file(
        &self,
        created: &LinuxCreatedFile,
        request: Option<&LinuxCreateFileRequest>,
    ) -> Result<()> {
        let item = created.item();
        let record = created.inode_record();
        let intent = created.mutation_intent();
        intent.validate_for_persistence().map_err(|_| {
            LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store returned an invalid durable mutation intent",
            }
        })?;
        if item.kind() != CloudItemKind::File || item.is_root() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store create did not return a non-root regular file",
            });
        }
        if item.content().is_none_or(|content| content.size() != 0) {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store create did not return empty file metadata",
            });
        }
        if record.key() != item.key() || record.inode() == crate::LINUX_ROOT_INODE {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store inode allocation did not match the created item",
            });
        }
        if item.key().scope() != self.inner.readonly.inode_table().scope() {
            return Err(LinuxCloudFilesError::ScopeMismatch);
        }
        let DesiredMutation::Create {
            scope,
            parent_id,
            name,
            kind,
        } = intent.desired()
        else {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store did not persist a create mutation intent",
            });
        };
        if intent.origin() != MutationOrigin::PlatformCommand
            || scope != item.key().scope()
            || item.parent_id() != Some(parent_id)
            || item.name() != name
            || *kind != CloudItemKind::File
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store mutation intent did not match the created file",
            });
        }
        if intent.session_generation() > self.inner.session_generation {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "recovered create intent came from a future mount generation",
            });
        }
        if let Some(request) = request
            && (request.parent_key().scope() != scope
                || request.parent_key().item_id() != parent_id
                || request.name() != name
                || request.session_generation() != intent.session_generation())
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store create acceptance did not match the native request",
            });
        }
        crate::validate_linux_name(item.name())?;
        Ok(())
    }

    fn ensure_created_file_is_unique(
        &self,
        state: &WritableState,
        created: &LinuxCreatedFile,
        name_key: &(CloudItemId, String),
    ) -> Result<()> {
        let record = created.inode_record();
        if self
            .inner
            .readonly
            .inode_table()
            .by_key(created.item().key())
            .is_some()
            || self
                .inner
                .readonly
                .inode_table()
                .by_inode(record.inode())
                .is_some()
            || state.created_by_key.contains_key(created.item().key())
            || state.created_by_inode.contains_key(&record.inode())
            || state.created_by_name.contains_key(name_key)
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "namespace store returned a duplicate key, inode, or parent/name mapping",
            });
        }
        Ok(())
    }

    async fn restore_created_files(&self, created: Vec<LinuxCreatedFile>) -> Result<()> {
        for created in created {
            self.validate_created_file(&created, None)?;
            let parent_id = created
                .item()
                .parent_id()
                .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered create omitted its parent identity",
                })?
                .clone();
            let parent_key =
                CloudItemKey::new(created.item().key().scope().clone(), parent_id.clone());
            let parent_inode = self.inode_for_key(&parent_key).ok_or(
                LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered create parent omitted a restored inode record",
                },
            )?;
            let parent = self.getattr(parent_inode, None).await?;
            if parent.attributes().kind() != crate::LinuxNodeKind::Directory {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered create parent was not a directory",
                });
            }
            let mut state = lock(&self.inner.state);
            let name_key = state
                .namespace_by_key
                .get(created.item().key())
                .and_then(|inode| state.namespace_by_inode.get(inode))
                .and_then(|item| {
                    item.item().parent_id().map(|current_parent| {
                        (current_parent.clone(), item.item().name().to_owned())
                    })
                })
                .unwrap_or_else(|| (parent_id, created.item().name().to_owned()));
            self.ensure_created_file_is_unique(&state, &created, &name_key)?;
            let inode = created.inode_record().inode();
            state
                .created_by_key
                .insert(created.item().key().clone(), inode);
            state.created_by_name.insert(name_key, inode);
            state.created_by_inode.insert(inode, created);
        }
        Ok(())
    }

    fn restore_namespace_overlay(&self, overlay: LinuxNamespaceOverlay) -> Result<()> {
        let (items, tombstones) = overlay.into_parts();
        for item in items {
            self.validate_namespace_item(&item)?;
            let parent_id = item
                .item()
                .parent_id()
                .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "restored namespace item omitted its parent identity",
                })?
                .clone();
            let name_key = (parent_id, item.item().name().to_owned());
            let mut state = lock(&self.inner.state);
            self.ensure_namespace_item_is_unique(&state, &item, &name_key, None)?;
            let inode = item.inode_record().inode();
            state
                .namespace_by_key
                .insert(item.item().key().clone(), inode);
            state.namespace_by_name.insert(name_key, inode);
            state.namespace_by_inode.insert(inode, item);
        }
        for tombstone in tombstones {
            if tombstone.key().scope() != self.inner.readonly.inode_table().scope()
                || tombstone.parent_key().scope() != self.inner.readonly.inode_table().scope()
                || tombstone.mutation_intent().session_generation() > self.inner.session_generation
            {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "restored namespace tombstone escaped the active scope or generation",
                });
            }
            let name_key = (
                tombstone.parent_key().item_id().clone(),
                tombstone.name().to_owned(),
            );
            let mut state = lock(&self.inner.state);
            let has_current_item = state.namespace_by_key.contains_key(tombstone.key());
            if matches!(
                tombstone.mutation_intent().desired(),
                DesiredMutation::Delete { key } if key == tombstone.key()
            ) || !has_current_item
                && tombstone.mutation_intent().desired().existing_item_key()
                    != Some(tombstone.key())
            {
                state.deleted_keys.insert(tombstone.key().clone());
            }
            if state
                .tombstones_by_name
                .insert(name_key, tombstone)
                .is_some()
            {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "restored namespace overlay contained duplicate tombstones",
                });
            }
        }
        Ok(())
    }

    fn item_key(&self, inode: LinuxInode) -> Result<CloudItemKey> {
        let state = lock(&self.inner.state);
        if let Some(created) = state.created_by_inode.get(&inode) {
            return Ok(created.item().key().clone());
        }
        if let Some(item) = state.namespace_by_inode.get(&inode) {
            return Ok(item.item().key().clone());
        }
        if let Some(item) = state.remote_by_inode.get(&inode) {
            return Ok(item.item().key().clone());
        }
        self.inner
            .readonly
            .inode_table()
            .by_inode(inode)
            .map(|record| record.key().clone())
            .ok_or(LinuxCloudFilesError::UnknownInode { inode: inode.get() })
    }

    fn restore_dirty_snapshots(&self, snapshots: Vec<LocalContentSnapshot>) -> Result<()> {
        let mut state = lock(&self.inner.state);
        for snapshot in snapshots {
            if snapshot.item_key().scope() != self.inner.readonly.inode_table().scope() {
                return Err(LinuxCloudFilesError::ScopeMismatch);
            }
            let inode = match self
                .inner
                .readonly
                .inode_table()
                .by_key(snapshot.item_key())
            {
                Some(record) => record.inode(),
                None => state
                    .created_by_key
                    .get(snapshot.item_key())
                    .or_else(|| state.namespace_by_key.get(snapshot.item_key()))
                    .or_else(|| state.remote_by_key.get(snapshot.item_key()))
                    .copied()
                    .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                        reason:
                            "writeback recovery returned an item without a restored inode record",
                    })?,
            };
            if inode == crate::LINUX_ROOT_INODE {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "writeback recovery returned a dirty snapshot for the mount root",
                });
            }
            if state
                .dirty_snapshots
                .insert(inode, snapshot.clone())
                .is_some_and(|previous| previous != snapshot)
            {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "writeback recovery returned conflicting snapshots for one inode",
                });
            }
        }
        Ok(())
    }

    fn validate_opened_session(
        &self,
        inode: LinuxInode,
        key: &CloudItemKey,
        session: &LinuxWriteSession,
        recovered: bool,
    ) -> Result<()> {
        if session.key() != key {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback store opened a different stable item identity",
            });
        }
        if session.session_generation() != self.inner.session_generation {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback store opened a session for a different mount generation",
            });
        }
        let Some(snapshot) = session.snapshot() else {
            if recovered {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered writeback session omitted its immutable dirty snapshot",
                });
            }
            return Ok(());
        };
        if snapshot.item_key() != key || snapshot.size() != session.size() {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback session snapshot did not match its item or logical size",
            });
        }
        let mut state = lock(&self.inner.state);
        match state.dirty_snapshots.get(&inode) {
            Some(current) if current.generation() > snapshot.generation() => {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered writeback session regressed the active dirty generation",
                });
            }
            Some(current)
                if current.generation() == snapshot.generation() && current != snapshot =>
            {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "one dirty generation identified different immutable snapshots",
                });
            }
            _ => {
                state.dirty_snapshots.insert(inode, snapshot.clone());
            }
        }
        Ok(())
    }

    fn apply_commit(
        &self,
        handle: LinuxFileHandle,
        inode: LinuxInode,
        commit: LinuxWriteCommit,
        minimum_size: Option<u64>,
        allow_same_generation: bool,
    ) -> Result<()> {
        let snapshot = commit.into_snapshot();
        let expected_key = self.item_key(inode)?;
        if snapshot.item_key() != &expected_key {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback commit returned a different stable item identity",
            });
        }
        if minimum_size.is_some_and(|minimum| snapshot.size() < minimum) {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback commit size omitted accepted bytes",
            });
        }
        let mut state = lock(&self.inner.state);
        let (opened_inode, previous_generation) = match state.files.get(&handle) {
            Some(OpenFile::Writeback {
                inode, generation, ..
            }) => (*inode, *generation),
            None => return Err(LinuxCloudFilesError::StaleHandle),
        };
        if opened_inode != inode {
            return Err(LinuxCloudFilesError::StaleHandle);
        }
        if previous_generation.is_some_and(|previous| {
            snapshot.generation() < previous
                || (!allow_same_generation && snapshot.generation() == previous)
        }) {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback commit did not advance the immutable local generation",
            });
        }
        let current = state.dirty_snapshots.get(&inode).cloned();
        if allow_same_generation
            && current.as_ref().is_some_and(|current| {
                snapshot.generation() == current.generation() && current != &snapshot
            })
        {
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "one dirty generation identified different immutable snapshots",
            });
        }
        if current.as_ref().is_some_and(|current| {
            snapshot.generation() < current.generation()
                || (!allow_same_generation && snapshot.generation() == current.generation())
        }) {
            if allow_same_generation
                && current
                    .as_ref()
                    .is_some_and(|current| snapshot.generation() < current.generation())
            {
                return Ok(());
            }
            return Err(LinuxCloudFilesError::InvalidBackendResponse {
                reason: "writeback commit did not advance the active dirty generation",
            });
        }
        let Some(OpenFile::Writeback {
            size, generation, ..
        }) = state.files.get_mut(&handle)
        else {
            return Err(LinuxCloudFilesError::StaleHandle);
        };
        *size = snapshot.size();
        *generation = Some(snapshot.generation());
        if current
            .as_ref()
            .is_none_or(|current| snapshot.generation() >= current.generation())
        {
            state.dirty_snapshots.insert(inode, snapshot);
        }
        Ok(())
    }
}
