//! Durable local-write staging for the Linux FUSE adapter.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use aster_forge_cloud_files_core::{
    CloudFilesBackend, CloudItemId, CloudItemKey, CloudItemKind, CloudScope, ContentRevision,
    DesiredMutation, LocalContentGeneration, LocalContentSnapshot, MutationOrigin,
    SessionGeneration, StoreResult,
};
use async_trait::async_trait;
use bytes::Bytes;

use crate::{
    LinuxCloudFilesError, LinuxCreateFileAcceptance, LinuxCreateFileRequest, LinuxCreatedFile,
    LinuxDirectoryEntry, LinuxDirectoryHandle, LinuxDirectorySnapshot, LinuxFileHandle, LinuxInode,
    LinuxNamespaceMutationStore, LinuxNode, LinuxReadOnlyEngine, Result,
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
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the exact remote content revision hydrated into staging.
    pub const fn base_revision(&self) -> &ContentRevision {
        &self.base_revision
    }

    /// Returns the exact hydrated byte length.
    pub const fn base_size(&self) -> u64 {
        self.base_size
    }

    /// Returns the active mount session generation that owns this open.
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
    pub const fn id(&self) -> &LinuxWriteSessionId {
        &self.id
    }

    /// Returns the item whose bytes are staged.
    pub const fn key(&self) -> &CloudItemKey {
        &self.key
    }

    /// Returns the current logical staging size.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the mount session generation bound to this writeback session.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }

    /// Returns the immutable dirty snapshot used to reopen this session, when present.
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
    pub const fn new(snapshot: LocalContentSnapshot) -> Self {
        Self { snapshot }
    }

    /// Returns the immutable dirty snapshot that is safe for upload recovery.
    pub const fn snapshot(&self) -> &LocalContentSnapshot {
        &self.snapshot
    }

    /// Consumes the commit into its immutable snapshot.
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

#[derive(Default)]
struct WritableState {
    next: u64,
    files: HashMap<LinuxFileHandle, OpenFile>,
    directories: HashMap<LinuxDirectoryHandle, LinuxDirectorySnapshot>,
    dirty_snapshots: HashMap<LinuxInode, LocalContentSnapshot>,
    created_by_inode: HashMap<LinuxInode, LinuxCreatedFile>,
    created_by_key: HashMap<CloudItemKey, LinuxInode>,
    created_by_name: HashMap<(CloudItemId, String), LinuxInode>,
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
    pub async fn activate(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        session_generation: SessionGeneration,
    ) -> Result<Self> {
        Self::activate_inner(readonly, store, None, session_generation).await
    }

    /// Activates writeback plus a durable namespace port for native regular-file creation.
    pub async fn activate_with_namespace<N>(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        namespace_store: Arc<N>,
        session_generation: SessionGeneration,
    ) -> Result<Self>
    where
        N: LinuxNamespaceMutationStore + 'static,
    {
        Self::activate_inner(readonly, store, Some(namespace_store), session_generation).await
    }

    async fn activate_inner(
        readonly: LinuxReadOnlyEngine<B>,
        store: Arc<S>,
        namespace_store: Option<Arc<dyn LinuxNamespaceMutationStore>>,
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
        let engine = Self {
            inner: Arc::new(WritableEngineInner {
                readonly,
                store,
                namespace_store,
                session_generation,
                state: Mutex::new(WritableState::default()),
            }),
        };
        engine.restore_created_files(created).await?;
        engine.restore_dirty_snapshots(snapshots)?;
        Ok(engine)
    }

    /// Returns the underlying read-only engine used for metadata and directory operations.
    pub fn readonly(&self) -> &LinuxReadOnlyEngine<B> {
        &self.inner.readonly
    }

    /// Returns the durable mount generation that owns this engine's writeback sessions.
    pub fn session_generation(&self) -> SessionGeneration {
        self.inner.session_generation
    }

    /// Loads attributes, overlaying the current staged size when a writable handle is supplied.
    pub async fn getattr(
        &self,
        inode: LinuxInode,
        handle: Option<LinuxFileHandle>,
    ) -> Result<LinuxNode> {
        let created = lock(&self.inner.state)
            .created_by_inode
            .get(&inode)
            .cloned();
        let node = match created {
            Some(created) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            None => self.inner.readonly.getattr(inode).await?,
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
    pub async fn lookup(&self, parent: LinuxInode, name: &str) -> Result<LinuxNode> {
        crate::validate_linux_name(name)?;
        let parent_node = self.getattr(parent, None).await?;
        if parent_node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let parent_key = parent_node.key();
        let created = {
            let state = lock(&self.inner.state);
            state
                .created_by_name
                .get(&(parent_key.item_id().clone(), name.to_owned()))
                .and_then(|inode| state.created_by_inode.get(inode))
                .cloned()
        };
        let node = match created {
            Some(created) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            None => self.inner.readonly.lookup(parent, name).await?,
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
    pub async fn open_file(
        &self,
        inode: LinuxInode,
        access: LinuxFileAccess,
    ) -> Result<LinuxFileHandle> {
        let created = lock(&self.inner.state)
            .created_by_inode
            .get(&inode)
            .cloned();
        let node = match created.as_ref() {
            Some(created) => self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?,
            None => self.inner.readonly.getattr(inode).await?,
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
                let (request, content) = self
                    .inner
                    .readonly
                    .hydrate_for_write(inode, self.inner.session_generation)
                    .await?;
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
        self.accept_created_file(request, acceptance, handle)
    }

    /// Opens a directory snapshot that includes durable local creates accepted before this call.
    pub async fn open_directory(&self, inode: LinuxInode) -> Result<LinuxDirectoryHandle> {
        let node = self.getattr(inode, None).await?;
        if node.attributes().kind() != crate::LinuxNodeKind::Directory {
            return Err(LinuxCloudFilesError::NotDirectory);
        }
        let base_handle = self.inner.readonly.open_directory(inode).await?;
        let base = self.inner.readonly.directory_snapshot(inode, base_handle);
        let release = self.inner.readonly.release_directory(base_handle);
        let base = match (base, release) {
            (Ok(base), Ok(())) => base,
            (Err(error), _) | (Ok(_), Err(error)) => return Err(error),
        };

        let mut entries = base.entries().to_vec();
        let mut local = {
            let state = lock(&self.inner.state);
            state
                .created_by_inode
                .values()
                .filter(|created| created.item().parent_id() == Some(node.key().item_id()))
                .cloned()
                .collect::<Vec<_>>()
        };
        local.sort_by(|left, right| left.item().name().cmp(right.item().name()));
        for created in local {
            if entries
                .iter()
                .any(|entry| entry.name() == created.item().name())
            {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "durable local create collided with a backend directory entry",
                });
            }
            let child = self
                .inner
                .readonly
                .node_from_item_and_record(created.item(), created.inode_record())?;
            entries.push(LinuxDirectoryEntry::from_node(
                created.item().name().to_owned(),
                &child,
            ));
        }
        let snapshot =
            LinuxDirectorySnapshot::with_entries(base.directory(), base.parent(), entries);
        let mut state = lock(&self.inner.state);
        let handle = state.allocate_directory()?;
        state.directories.insert(handle, snapshot);
        Ok(handle)
    }

    /// Returns the immutable writable-directory snapshot for one handle.
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

    /// Releases one writable-directory snapshot handle.
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
            _ => Err(LinuxCloudFilesError::StaleHandle),
        }
    }

    /// Durably applies a positioned write before reporting accepted bytes to FUSE.
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
            Some(OpenFile::Writeback { .. }) => Err(LinuxCloudFilesError::StaleHandle),
            None => Err(LinuxCloudFilesError::StaleHandle),
        }
    }

    /// Releases one file handle and its product-owned writeback session.
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
        request: LinuxCreateFileRequest,
        acceptance: LinuxCreateFileAcceptance,
        handle: LinuxFileHandle,
    ) -> Result<(LinuxNode, LinuxFileHandle)> {
        let (created, session) = acceptance.into_parts();
        self.validate_created_file(&created, Some(&request))?;
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
            let parent_record = self
                .inner
                .readonly
                .inode_table()
                .by_key(&parent_key)
                .ok_or(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered create parent omitted a restored inode record",
                })?;
            let parent = self.inner.readonly.getattr(parent_record.inode()).await?;
            if parent.attributes().kind() != crate::LinuxNodeKind::Directory {
                return Err(LinuxCloudFilesError::InvalidBackendResponse {
                    reason: "recovered create parent was not a directory",
                });
            }
            let name_key = (parent_id, created.item().name().to_owned());
            let mut state = lock(&self.inner.state);
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

    fn item_key(&self, inode: LinuxInode) -> Result<CloudItemKey> {
        let state = lock(&self.inner.state);
        if let Some(created) = state.created_by_inode.get(&inode) {
            return Ok(created.item().key().clone());
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
